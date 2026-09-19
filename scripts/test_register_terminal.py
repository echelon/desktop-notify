import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import notification_origin as origins
import register_terminal as register


class RegistrationTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.path = Path(directory.name) / "terminal-origins.json"
        self.addCleanup(patch.stopall)
        patch.object(origins, "REGISTRATIONS", self.path).start()
        self.origin = {"terminal_app": "com.mitchellh.ghostty", "app_pid": 123,
                       "tty": "/dev/ttys001", "tmux_socket": "/tmp/tmux", "tmux_pane": "%3"}

    def register(self, terminal=False):
        with patch.object(origins, "capture_origin", return_value=self.origin), \
             patch.object(register.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "window-id\nterminal-id\n", "")):
            return register.register(terminal)

    def test_window_registration_is_scoped_to_tty_and_app_lifetime(self):
        result = self.register()
        self.assertEqual(result["window_id"], "window-id")
        self.assertNotIn("tmux_pane", result)
        self.assertNotIn("terminal_id", result)
        self.assertEqual(origins.registered_origin(self.origin), {"window_id": "window-id"})
        self.assertEqual(origins.registered_origin({**self.origin, "tty": "/dev/ttys002"}), {})
        self.assertEqual(origins.registered_origin({**self.origin, "app_pid": 456}), {})
        self.assertEqual(origins.registered_origin({**self.origin, "terminal_app": "com.apple.Terminal"}), {})

    def test_terminal_registration_is_optional(self):
        self.assertEqual(self.register(True)["terminal_id"], "terminal-id")
        self.assertEqual(origins.registered_origin(self.origin)["terminal_id"], "terminal-id")

    def test_a_new_registration_preserves_other_windows(self):
        self.path.write_text(json.dumps({"456:/dev/ttys002": {"window_id": "other"}}))
        self.register()
        self.assertEqual(json.loads(self.path.read_text())["456:/dev/ttys002"]["window_id"], "other")

    def test_denied_or_background_window_does_not_save_mapping(self):
        with patch.object(origins, "capture_origin", return_value=self.origin), \
             patch.object(register.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "Bring Ghostty forward")):
            with self.assertRaisesRegex(RuntimeError, "Bring Ghostty forward"):
                register.register()
        self.assertFalse(self.path.exists())

    def test_broken_cache_does_not_break_hooks(self):
        self.path.write_text("bad json")
        self.assertEqual(origins.registered_origin(self.origin), {})

    def test_hook_adds_window_mapping_and_respects_explicit_override(self):
        self.register()
        with patch.object(origins.sys, "platform", "darwin"), \
             patch.object(origins.os, "getppid", return_value=42), \
             patch.object(origins, "process_table", return_value={42: (1, "ttys001", "codex")}), \
             patch.object(origins, "application", return_value={"terminal_app": "com.mitchellh.ghostty", "app_pid": 123}):
            self.assertEqual(origins.capture_origin({})["window_id"], "window-id")
            self.assertEqual(origins.capture_origin({"NOTIFY_WINDOW_ID": "override"})["window_id"], "override")


if __name__ == "__main__":
    unittest.main()
