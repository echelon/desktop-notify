import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import notification_context as context


class ContextTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.repo = self.root / 'sample repo'
        self.repo.mkdir()
        (self.repo / '.git').mkdir()

    def transcript(self, *items):
        path = self.root / 'transcript.jsonl'
        path.write_text('\n'.join(json.dumps(item) for item in items) + '\n')
        return str(path)

    def message(self, role, text, phase=None):
        return {'type': 'response_item', 'payload': {'type': 'message', 'role': role, 'phase': phase,
                'content': [{'type': 'input_text' if role == 'user' else 'output_text', 'text': text}]}}

    def test_repo_manifest_and_cwd_are_discovered_from_a_nested_directory(self):
        child = self.repo / 'src' / 'two  spaces'
        child.mkdir(parents=True)
        (self.repo / 'package.json').write_text(json.dumps({'name': 'Sample', 'description': 'A useful project.'}))
        result = context.capture_context({'cwd': str(child)}, {})
        self.assertEqual(result, {'cwd': str(child), 'repo_name': 'Sample', 'repo_description': 'A useful project.'})

    def test_workspace_readme_and_worktree_pointer_fallback(self):
        (self.repo / '.git').rmdir()
        (self.repo / '.git').write_text('gitdir: /somewhere/else')
        (self.repo / 'Cargo.toml').write_text('[workspace]\nmembers = ["crates/*"]\n')
        (self.repo / 'README.md').write_text('# Sample\n\n[![Build](badge.svg)](build)\n\nA **small** project\nfor [agents](https://example.test).\n\n## Setup\nDo not use this as a description.\n')
        result = context.repository_context(str(self.repo))
        self.assertEqual(result['repo_name'], 'sample repo')
        self.assertEqual(result['repo_description'], 'A small project for agents.')

    def test_missing_and_malformed_repo_metadata_are_optional(self):
        (self.repo / 'package.json').write_text('not json')
        (self.repo / 'Cargo.toml').write_text('[package]\nname = "rust-project"\ndescription = "A Rust app."\n')
        self.assertEqual(context.repository_context(str(self.repo))['repo_name'], 'rust-project')
        self.assertEqual(context.capture_context({}, {}), {})
        self.assertEqual(context.capture_context({'cwd': '/nonexistent/project'}, {}), {'cwd': '/nonexistent/project'})
        self.assertEqual(context.capture_context({'cwd': str(self.root)}, {}), {'cwd': str(self.root)})

    def test_python_project_manifest(self):
        (self.repo / 'pyproject.toml').write_text('[project]\nname = "python-project"\ndescription = "Python tools."\n')
        self.assertEqual(context.repository_context(str(self.repo)), {'repo_name': 'python-project', 'repo_description': 'Python tools.'})

    def test_transcript_uses_latest_request_and_its_first_agent_summary(self):
        path = self.transcript(
            self.message('user', 'Old task'),
            self.message('assistant', 'Old interpretation', 'commentary'),
            self.message('user', '<environment_context>Ignore this bootstrap</environment_context>'),
            self.message('user', 'Add per-session context.'),
            self.message('assistant', 'I will add session metadata and a compact UI.', 'commentary'),
            self.message('assistant', 'Now running tests.', 'commentary'),
            self.message('assistant', 'Done.', 'final_answer'),
        )
        self.assertEqual(context.transcript_context(path), {'current_ask': 'Add per-session context.', 'work_arc': 'I will add session metadata and a compact UI.'})
        with Path(path).open('a') as stream:
            stream.write(json.dumps(self.message('user', 'Switch to the server API.')) + '\n')
        self.assertEqual(context.transcript_context(path), {'current_ask': 'Switch to the server API.'})

    def test_claude_code_transcript_uses_the_latest_typed_prompt(self):
        path = self.transcript(
            {'type': 'user', 'message': {'role': 'user', 'content': 'Add a snooze button.'}},
            {'type': 'assistant', 'message': {'content': [{'type': 'text', 'text': 'On it.'}]}},
            {'type': 'user', 'message': {'content': [{'type': 'tool_result', 'content': 'ok'}]}},
            {'type': 'user', 'message': {'content': [{'type': 'text', 'text': 'Rebuild the app.<system-reminder>x</system-reminder>'}]}},
            {'type': 'user', 'isMeta': True, 'message': {'content': 'Skill instructions'}},
            {'type': 'user', 'message': {'content': '<command-name>/model</command-name>'}},
            {'type': 'user', 'message': {'content': '<local-command-stdout>Set model</local-command-stdout>'}},
        )
        self.assertEqual(context.transcript_context(path), {'current_ask': 'Rebuild the app.'})

    def test_explicit_goal_can_update_the_arc(self):
        path = self.transcript(self.message('user', 'Fix login.'),
            {'type': 'response_item', 'payload': {'type': 'function_call', 'name': 'functions.update_plan', 'arguments': json.dumps({'explanation': 'Repair the login flow.'})}},
            {'type': 'response_item', 'payload': {'type': 'function_call', 'name': 'functions.create_goal', 'arguments': json.dumps({'objective': 'Make authentication reliable.'})}})
        self.assertEqual(context.transcript_context(path)['work_arc'], 'Make authentication reliable.')

    def test_explicit_context_overrides_discovery_and_supports_clearing(self):
        path = self.transcript(self.message('user', 'Original request'))
        result = context.capture_context({'cwd': str(self.repo), 'transcript_path': path,
            'context': {'work_arc': 'Broader goal', 'repo_description': '', 'current_ask': 'Current task'}},
            {'NOTIFY_WORK_ARC': 'Earlier goal', 'NOTIFY_REPO_NAME': 'Friendly project'})
        self.assertEqual(result['work_arc'], 'Broader goal')
        self.assertEqual(result['current_ask'], 'Current task')
        self.assertEqual(result['repo_name'], 'Friendly project')
        self.assertEqual(result['repo_description'], '')

    def test_bounds_bad_records_and_transcript_unavailability(self):
        self.assertEqual(context.transcript_context('/no/such/transcript'), {})
        self.assertEqual(context.transcript_context(str(self.root)), {})
        path = Path(self.transcript(self.message('user', 'z' * 3000)))
        with path.open('a') as stream:
            stream.write('bad json\nnull\n{"type":"response_item","payload":null}\n')
        self.assertEqual(len(context.transcript_context(str(path))['current_ask']), 2000)
        with patch.object(context, 'MAX_TRANSCRIPT_BYTES', 50):
            self.assertEqual(context.transcript_context(str(path)), {})
        self.assertNotIn('cwd', context.capture_context({'cwd': '/bad\x00path'}, {}))
        self.assertEqual(context.capture_context({'context': {'repo_name': 'x' * 500}}, {})['repo_name'], 'x' * 199 + '…')


if __name__ == '__main__':
    unittest.main()
