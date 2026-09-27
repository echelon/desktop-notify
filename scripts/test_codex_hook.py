import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import codex_hook as hook


class HookTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.event_log = Path(directory.name) / "hook-events.jsonl"
        patch.object(hook, "EVENT_LOG", self.event_log).start()
        patch.dict(hook.os.environ, {}, clear=True).start()
        self.context = patch.object(hook, "capture_context", return_value={}).start()
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

    def test_empty_origin_explicitly_replaces_stale_terminal_hints(self):
        self.origin.return_value = {}
        posts, _ = self.run_hook({"hook_event_name": "Stop", "session_id": "daemon-session"})
        self.assertEqual(posts[0][1]["origin"], {})

    def test_session_id_is_attached_to_every_notification_event(self):
        for event in [
            {"hook_event_name": "Stop", "last_assistant_message": "Done."},
            {"hook_event_name": "Stop", "last_assistant_message": "Which option?"},
            {"hook_event_name": "PermissionRequest", "tool_input": {"justification": "Approve?"}},
            {"hook_event_name": "PreToolUse", "tool_name": "request_user_input"},
            {"hook_event_name": "PreToolUse", "tool_name": "request_user_input_async"},
        ]:
            for session_id in ("codex-a", "codex-b"):
                with self.subTest(event=event, session=session_id), \
                     patch("sys.stdin", io.StringIO(json.dumps({**event, "session_id": session_id}))), \
                     contextlib.redirect_stdout(io.StringIO()), patch.object(hook, "ensure_server"), \
                     patch.object(hook, "http", return_value={"id": "test"}) as post:
                    hook.main()
                    self.assertEqual(post.call_args.args[1]["session_id"], session_id)

    def test_claude_stop_falls_back_to_the_last_assistant_text_in_its_transcript(self):
        import tempfile
        records = [
            {"type": "assistant", "message": {"content": [{"type": "text", "text": "Earlier reply"}]}},
            {"type": "user", "message": {"content": "next"}},
            {"type": "assistant", "message": {"content": [{"type": "text", "text": "Added **global** sound."}]}},
            {"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Bash"}]}},
        ]
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl") as transcript:
            transcript.write("not json\n" + "\n".join(json.dumps(r) for r in records) + "\n")
            transcript.flush()
            event = {"hook_event_name": "Stop", "session_id": "claude-a", "transcript_path": transcript.name}
            endpoint, payload = hook.notification_for(event)
            self.assertEqual(endpoint, "/all_tasks_finished")
            self.assertEqual(payload["message"], "Added global sound.")
            # An explicit final message still wins over the transcript.
            _, payload = hook.notification_for({**event, "last_assistant_message": "Explicit."})
            self.assertEqual(payload["message"], "Explicit.")
        missing = {"hook_event_name": "Stop", "transcript_path": "/nonexistent/transcript.jsonl"}
        self.assertEqual(hook.notification_for(missing)[1]["message"], "The agent finished its turn.")

    def run_hook(self, event, version=4):
        """Run main() against a fake service; returns [(endpoint, payload)] posted."""
        posts = []
        def http(path, payload=None, timeout=2):
            if path == "/health":
                return {"service": "desktop-notify", "api_version": version}
            posts.append((path, payload))
            return {"updated": True} if path == "/working" else {"id": "x"}
        out = io.StringIO()
        with patch("sys.stdin", io.StringIO(json.dumps(event))), contextlib.redirect_stdout(out), \
             patch.object(hook, "ensure_server") as ensure, patch.object(hook, "http", side_effect=http):
            hook.main()
        self.assertEqual(json.loads(out.getvalue()), {})
        return posts, ensure

    def test_prompt_submission_marks_the_session_working_without_starting_the_service(self):
        self.origin.return_value = {"terminal_app": "ghostty"}
        posts, ensure = self.run_hook({"hook_event_name": "UserPromptSubmit", "session_id": "s1",
                                       "cwd": "/work/app", "prompt": "Fix **login**\nthen tests"})
        self.assertEqual(posts[0][0], "/working")
        self.assertEqual(posts[0][1]["title"], "app: Fix login")
        self.assertEqual(posts[0][1]["origin"], {"terminal_app": "ghostty"})
        ensure.assert_not_called()

    def test_post_tool_use_is_minimal_and_only_resumes_waiting_rows(self):
        posts, ensure = self.run_hook({"hook_event_name": "PostToolUse", "session_id": "s1", "tool_name": "Bash"})
        self.assertEqual(posts, [("/working", {"session_id": "s1", "only_if_waiting": True})])
        ensure.assert_not_called()
        self.context.assert_not_called()
        self.origin.assert_not_called()
        posts, _ = self.run_hook({"hook_event_name": "PostToolUse", "session_id": "s1", "tool_use_id": "t1"})
        self.assertEqual(posts[0][1]["tool_use_id"], "t1")
        question = hook.notification_for({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion", "tool_use_id": "t2"})
        self.assertEqual(question[1]["tool_use_id"], "t2")
        # No session to update, or an older service without task states: skip quietly.
        self.assertEqual(self.run_hook({"hook_event_name": "PostToolUse"})[0], [])
        self.assertEqual(self.run_hook({"hook_event_name": "PostToolUse", "session_id": "s1"}, version=3)[0], [])

    def test_stop_failure_reports_a_failed_task_and_degrades_on_older_services(self):
        event = {"hook_event_name": "StopFailure", "session_id": "s1", "error": "rate_limit"}
        posts, ensure = self.run_hook(event)
        self.assertEqual(posts[0][0], "/task_failed")
        self.assertEqual(posts[0][1]["message"], "rate_limit")
        ensure.assert_called_once()
        self.assertEqual(self.run_hook(event, version=3)[0][0][0], "/all_tasks_finished")

    def test_async_question_return_does_not_mean_the_user_answered(self):
        for name in ("request_user_input_async", "functions.request_user_input_async",
                     "mcp__questions__request_user_input_async"):
            with self.subTest(tool=name):
                event = {"session_id": "s1", "tool_name": name, "tool_use_id": "question-1"}
                posts, _ = self.run_hook({**event, "hook_event_name": "PreToolUse"})
                self.assertEqual(posts[0][0], "/awaiting_user_input")
                self.assertEqual(posts[0][1]["tool_use_id"], "question-1")
                posts, ensure = self.run_hook({**event, "hook_event_name": "PostToolUse"})
                self.assertEqual(posts, [])
                ensure.assert_not_called()
        posts, _ = self.run_hook({"hook_event_name": "UserPromptSubmit", "session_id": "s1", "prompt": "Continue"})
        self.assertEqual(posts[0][0], "/working")
        # Blocking questions still resume on their matching tool completion.
        posts, _ = self.run_hook({"hook_event_name": "PostToolUse", "session_id": "s1",
                                 "tool_name": "functions.request_user_input", "tool_use_id": "question-2"})
        self.assertEqual(posts, [("/working", {"session_id": "s1", "only_if_waiting": True,
                                             "tool_use_id": "question-2"})])

    def test_each_hook_decision_is_recorded_with_its_reason(self):
        self.run_hook({"hook_event_name": "Stop", "session_id": "s1",
                       "last_assistant_message": "Done.\n\nWaiting for your approval of the plan."})
        self.run_hook({"hook_event_name": "PostToolUse", "session_id": "s1", "tool_name": "Bash", "tool_use_id": "t"})
        stop, tool = [json.loads(line) for line in self.event_log.read_text().splitlines()]
        self.assertEqual((stop["action"], stop["closing"]), ("/awaiting_user_input", "Waiting for your approval of the plan."))
        self.assertEqual((tool["action"], tool["tool_name"], tool["updated"]), ("/working", "Bash", True))
        self.assertIn("ms", tool)
        with patch("sys.stdin", io.StringIO("not json")), contextlib.redirect_stdout(io.StringIO()):
            hook.main()
        self.assertIn("error", json.loads(self.event_log.read_text().splitlines()[-1]))

    def test_agent_is_detected_from_its_own_markers(self):
        detect = hook.detect_agent
        self.assertEqual(detect({}, {"CLAUDECODE": "1"}), "claude_code")
        self.assertEqual(detect({}, {"AI_AGENT": "claude-code_2-1-283_agent"}), "claude_code")
        self.assertEqual(detect({}, {"CODEX_THREAD_ID": "t"}), "codex")
        self.assertEqual(detect({"transcript_path": "/Users/x/.codex/sessions/2026/a.jsonl"}, {}), "codex")
        self.assertEqual(detect({"transcript_path": "/Users/x/.claude/projects/p/a.jsonl"}, {}), "claude_code")
        self.assertEqual(detect({}, {"NOTIFY_AGENT": "codex", "CLAUDECODE": "1"}), "codex")
        self.assertIsNone(detect({}, {"NOTIFY_AGENT": "$(evil)"}, argv=[]))
        # The installer's self-report wins over environment markers.
        self.assertEqual(detect({}, {"CLAUDECODE": "1"}, argv=["--agent", "codex"]), "codex")
        self.assertEqual(detect({}, {"CLAUDECODE": "1"}, argv=["--agent", "bogus"]), "claude_code")
        with patch.dict(hook.os.environ, {"CLAUDECODE": "1"}):
            posts, _ = self.run_hook({"hook_event_name": "Stop", "session_id": "s", "last_assistant_message": "Done."})
            tool, _ = self.run_hook({"hook_event_name": "PostToolUse", "session_id": "s"})
        self.assertEqual(posts[0][1]["agent"], "claude_code")
        self.assertNotIn("agent", tool[0][1])  # the row keeps its agent

    def test_event_log_rolls_over_to_its_newest_half(self):
        with patch.object(hook, "EVENT_LOG_LIMIT", 2000):
            for n in range(100):
                hook.record({"n": n, "pad": "x" * 40})
        numbers = [json.loads(line)["n"] for line in self.event_log.read_text().splitlines()]
        self.assertLessEqual(self.event_log.stat().st_size, 2000)
        self.assertEqual(numbers[-1], 99)
        self.assertEqual(numbers, list(range(numbers[0], 100)))

    def test_session_fallback_and_legacy_payload(self):
        event = {"hook_event_name": "Stop"}
        self.assertNotIn("session_id", hook.notification_for(event)[1])
        with patch.dict(hook.os.environ, {"CODEX_THREAD_ID": "shell-thread"}):
            self.assertEqual(hook.notification_for(event)[1]["session_id"], "shell-thread")
            self.assertEqual(hook.notification_for({**event, "session_id": "event-thread"})[1]["session_id"], "event-thread")

    def test_optional_context_reaches_each_notification_request(self):
        self.context.return_value = {"cwd": "/workspace/demo", "current_ask": "Fix login", "work_arc": "Reliable authentication"}
        for event in [
            {"hook_event_name": "Stop"},
            {"hook_event_name": "PermissionRequest"},
            {"hook_event_name": "PreToolUse", "tool_name": "request_user_input"},
        ]:
            with patch("sys.stdin", io.StringIO(json.dumps(event))), contextlib.redirect_stdout(io.StringIO()), \
                 patch.object(hook, "ensure_server"), patch.object(hook, "http", return_value={"id": "test"}) as post:
                hook.main()
                self.assertEqual(post.call_args.args[1]["context"], self.context.return_value)

    def test_context_failure_cannot_suppress_alert_or_origin(self):
        self.context.side_effect = OSError("Transcript unavailable")
        self.origin.return_value = {"terminal_app": "ghostty"}
        with patch("sys.stdin", io.StringIO('{"hook_event_name":"Stop"}')), contextlib.redirect_stdout(io.StringIO()), \
             patch.object(hook, "ensure_server"), patch.object(hook, "http", return_value={"id": "test"}) as post:
            hook.main()
        self.assertNotIn("context", post.call_args.args[1])
        self.assertEqual(post.call_args.args[1]["origin"], self.origin.return_value)

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

    def test_only_the_closing_paragraph_decides_a_finished_turn_is_waiting(self):
        summary = ("Each task now has its own state.\n\n- If one tool is waiting for your approval and "
                   "another finishes, the prompt stays open.\n- Should rows dim?\n\nNothing is committed.")
        self.assertEqual(hook.notification_for({"hook_event_name": "Stop", "last_assistant_message": summary})[0],
                         "/all_tasks_finished")
        asks = summary + "\n\nShould I commit these changes?"
        self.assertEqual(hook.notification_for({"hook_event_name": "Stop", "last_assistant_message": asks})[0],
                         "/awaiting_user_input")

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
