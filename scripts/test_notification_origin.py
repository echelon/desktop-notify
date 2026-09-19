import unittest
from unittest.mock import patch

import notification_origin as origin


class OriginTests(unittest.TestCase):
    def test_app_only_environment_needs_no_window_or_pane(self):
        with patch.object(origin, "process_table", return_value={}), patch.object(origin.sys, "platform", "darwin"):
            result = origin.capture_origin({"TERM_PROGRAM": "ghostty"})
        self.assertEqual(result["terminal_app"], "com.mitchellh.ghostty")
        self.assertNotIn("window_id", result)
        self.assertNotIn("terminal_id", result)
        self.assertNotIn("tmux_pane", result)

    def test_tmux_uses_attached_client_instead_of_server_ancestry(self):
        env = {"TMUX": "/tmp/tmux-test,90,1", "TMUX_PANE": "%5", "ITERM_SESSION_ID": "stale:id"}
        def run(args):
            if "display-message" in args: return "$1"
            if "list-clients" in args: return "100|/dev/ttys001|$1|5\n101|/dev/ttys002|$1|10\n102|/dev/ttys003|$2|20"
            return ""
        with patch.object(origin, "command", side_effect=run), patch.object(origin, "process_table", return_value={}), \
             patch.object(origin, "application", return_value={"terminal_app": "com.mitchellh.ghostty", "app_pid": 123}) as app, \
             patch.object(origin.sys, "platform", "darwin"):
            result = origin.capture_origin(env)
        self.assertEqual(app.call_args.args[1], 101)
        self.assertEqual(result["tmux_client"], "/dev/ttys002")
        self.assertEqual(result["tmux_pane"], "%5")
        self.assertEqual(result["app_pid"], 123)
        self.assertNotIn("terminal_id", result)

    def test_explicit_window_and_terminal_hints_are_preserved_as_data(self):
        env = {"TERM_PROGRAM": "ghostty", "NOTIFY_TERMINAL_ID": "some-uuid",
               "NOTIFY_WINDOW_TITLE": 'literal "quote"; $(not-a-command)'}
        with patch.object(origin, "process_table", return_value={}), patch.object(origin.sys, "platform", "darwin"):
            result = origin.capture_origin(env)
        self.assertEqual(result["terminal_id"], "some-uuid")
        self.assertEqual(result["window_title"], env["NOTIFY_WINDOW_TITLE"])

    def test_iterm_id_and_controlling_terminal(self):
        with patch.object(origin, "process_table", return_value={42: (1, "ttys002", "codex")}), \
             patch.object(origin.os, "getppid", return_value=42), patch.object(origin.sys, "platform", "darwin"):
            result = origin.capture_origin({"TERM_PROGRAM": "iTerm.app", "ITERM_SESSION_ID": "w0t0p0:stable-id"})
        self.assertEqual(result["terminal_id"], "stable-id")
        self.assertEqual(result["tty"], "/dev/ttys002")
        self.assertEqual(result["pid"], 42)

    def test_unavailable_tmux_is_optional(self):
        with patch.object(origin, "command", return_value=""):
            result, pid = origin.tmux_context({"TMUX": "/tmp/tmux-test,90,1", "TMUX_PANE": "%5"})
        self.assertIsNone(pid)
        self.assertEqual(result, {"tmux_socket": "/tmp/tmux-test", "tmux_pane": "%5"})

    def test_ancestry_stops_on_a_cycle(self):
        self.assertEqual(len(list(origin.ancestry({2: (3, "?", "sh"), 3: (2, "?", "sh")}, 2))), 2)


if __name__ == "__main__":
    unittest.main()
