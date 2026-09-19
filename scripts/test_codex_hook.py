import contextlib
import io
import json
import unittest
from unittest.mock import patch

import codex_hook as hook


class HookTests(unittest.TestCase):
    def setUp(self):
        self.origin = patch.object(hook, "capture_origin", return_value=None).start()
        self.addCleanup(patch.stopall)

    def test_origin_is_attached_to_the_http_request(self):
        self.origin.return_value = {"terminal_app": "ghostty"}
        with patch("sys.stdin", io.StringIO('{"hook_event_name":"Stop"}')), contextlib.redirect_stdout(io.StringIO()), \
             patch.object(hook, "ensure_server"), patch.object(hook, "http", return_value={"id": "test"}) as post:
            hook.main()
        self.assertEqual(post.call_args.args[1]["origin"], {"terminal_app": "ghostty"})

    def test_origin_failure_does_not_suppress_notification(self):
        self.origin.side_effect = OSError("ps unavailable")
        with patch("sys.stdin", io.StringIO('{"hook_event_name":"Stop"}')), contextlib.redirect_stdout(io.StringIO()), \
             patch.object(hook, "ensure_server"), patch.object(hook, "http", return_value={"id": "test"}) as post:
            hook.main()
        self.assertNotIn("origin", post.call_args.args[1])

    def test_completion_has_outcome(self):
        endpoint, payload = hook.notification_for({"hook_event_name": "Stop", "cwd": "/tmp/my-project",
                                                  "last_assistant_message": "**Fixed login.**\nAll 12 tests pass."})
        self.assertEqual(endpoint, "/all_tasks_finished")
        self.assertEqual(payload["title"], "my-project: Fixed login.")
        self.assertIn("All 12 tests pass.", payload["message"])

    def test_both_question_tools_and_json_string_input(self):
        for name in ("request_user_input", "functions.request_user_input", "request_user_input_async"):
            endpoint, payload = hook.notification_for({"hook_event_name": "PreToolUse", "tool_name": name,
                "tool_input": json.dumps({"questions": [{"question": "Which database?"}, {"title": "Which region?"}]})})
            self.assertEqual(endpoint, "/awaiting_user_input")
            self.assertEqual(payload["message"], "Which database?\n\nWhich region?")

    def test_permission_does_not_execute_command(self):
        command = 'echo "$(touch /tmp/should-not-exist)"; `whoami`'
        endpoint, payload = hook.notification_for({"hook_event_name": "PermissionRequest", "tool_input": {
            "description": "Allow deployment?", "command": command}})
        self.assertEqual(endpoint, "/awaiting_user_input")
        self.assertIn("$(touch /tmp/should-not-exist)", payload["message"])

    def test_plain_text_question_is_awaiting(self):
        for message in ("Which option?", "I need your approval to continue."):
            self.assertEqual(hook.notification_for({"hook_event_name": "Stop", "last_assistant_message": message})[0],
                             "/awaiting_user_input")

    def test_unrelated_events_do_nothing(self):
        self.assertIsNone(hook.notification_for({"hook_event_name": "SubagentStop"}))
        self.assertIsNone(hook.notification_for({"hook_event_name": "PreToolUse", "tool_name": "Bash"}))

    def test_limits_and_unicode(self):
        _, payload = hook.notification_for({"hook_event_name": "Stop", "last_assistant_message": "✓" * 9000})
        self.assertLessEqual(len(payload["title"]), 200)
        self.assertEqual(len(payload["message"]), 4000)

    def test_running_server_skips_build(self):
        with patch.object(hook, "running", return_value=True), patch.object(hook, "ensure_desktop"), patch.object(hook.subprocess, "run") as build:
            hook.ensure_server()
            build.assert_not_called()

    def test_success_outputs_valid_hook_json(self):
        event = {"hook_event_name": "Stop", "last_assistant_message": "Done."}
        output = io.StringIO()
        with patch("sys.stdin", io.StringIO(json.dumps(event))), contextlib.redirect_stdout(output), \
             patch.object(hook, "ensure_server"), patch.object(hook, "http", return_value={"id": "test"}) as post:
            hook.main()
        self.assertEqual(json.loads(output.getvalue()), {})
        self.assertEqual(post.call_args.args[0], "/all_tasks_finished")

    def test_failure_is_visible_without_blocking_codex(self):
        output = io.StringIO()
        with patch("sys.stdin", io.StringIO('{"hook_event_name":"Stop"}')), contextlib.redirect_stdout(output), \
             patch.object(hook, "ensure_server", side_effect=RuntimeError("build failed")):
            hook.main()
        self.assertIn("build failed", json.loads(output.getvalue())["systemMessage"])


if __name__ == "__main__":
    unittest.main()
