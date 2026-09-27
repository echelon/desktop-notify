import json
import fcntl
import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import notification_origin as origin


class OriginTests(unittest.TestCase):
    def setUp(self):
        # Never script the real Ghostty or touch the real pairing file.
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.registrations = Path(directory.name) / "terminal-origins.json"
        patch.object(origin, "REGISTRATIONS", self.registrations).start()
        self.ghostty = patch.object(origin, "ghostty", return_value=None).start()
        self.addCleanup(patch.stopall)

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
            if "#{pid} #{session_id} #{window_id} #{window_index}" in args: return "90 $1 @4 2"
            if "#{session_name}" in args: return "work"
            if "#{window_name}" in args: return "claude\x07"
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
        self.assertEqual((result["tmux_server_pid"], result["tmux_session"], result["tmux_window"],
                          result["tmux_window_index"], result["tmux_session_name"], result["tmux_window_name"]),
                         (90, "$1", "@4", 2, "work", "claude"))
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

    def probe(self, answers):
        """Run the Ghostty probe against scripted lookups, recording TTY writes."""
        writes = []
        def lookup(*args):
            if args[0] == "title":
                return answers.get(("title", "marker")) if args[1] in [w.rstrip("\x07") for w in writes] else ""
            return answers.get(args)
        self.ghostty.side_effect = lookup
        with patch.object(origin.os, "open", return_value=9) as opened, \
             patch.object(origin.os, "write", side_effect=lambda fd, data: writes.append(data.decode().removeprefix("\x1b]2;"))), \
             patch.object(origin.os, "close"), patch.object(origin.time, "sleep"):
            result = origin.ghostty_origin({"terminal_app": "com.mitchellh.ghostty", "app_pid": 55, "tty": "/dev/ttys027"})
        return result, writes, opened

    def test_ghostty_probe_names_this_surface_and_restores_its_title(self):
        result, writes, opened = self.probe({
            ("titles",): "T1\tOther\nT2\tsession / \x1b]2;evil\x07title",
            ("title", "marker"): "W2\nTB2\nT2",
        })
        self.assertEqual(result, {"window_id": "W2", "tab_id": "TB2", "terminal_id": "T2"})
        self.assertEqual(opened.call_args.args[0], "/dev/ttys027")
        self.assertRegex(writes[0], r"^desktop-notify-[0-9a-f]{32}\x07$")
        # Restored as printable text; an embedded sequence cannot be replayed.
        self.assertEqual(writes[1], "session / ]2;evil" + "title\x07")
        saved = json.loads(self.registrations.read_text())["55:/dev/ttys027"]
        self.assertEqual(saved["terminal_id"], "T2")

    def test_cached_ghostty_surface_is_revalidated_without_writing_a_title(self):
        origin.save_registration("55:/dev/ttys027", {"terminal_app": "com.mitchellh.ghostty", "window_id": "W1", "terminal_id": "T2"})
        result, writes, _ = self.probe({("terminal", "T2"): "W9\nTB9\nT2"})
        self.assertEqual(result, {"window_id": "W9", "tab_id": "TB9", "terminal_id": "T2"})
        self.assertEqual(writes, [])

    def test_closed_cached_surface_is_probed_again(self):
        origin.save_registration("55:/dev/ttys027", {"terminal_app": "com.mitchellh.ghostty", "terminal_id": "gone"})
        result, writes, _ = self.probe({("terminal", "gone"): "", ("titles",): "T3\tnew", ("title", "marker"): "W3\nTB3\nT3"})
        self.assertEqual(result["terminal_id"], "T3")
        self.assertEqual(len(writes), 2)

    def test_parallel_probe_on_same_tty_never_overwrites_the_active_marker(self):
        key = "55:/dev/ttys027"
        lock_path = self.registrations.parent / f"ghostty-probe-{hashlib.sha256(key.encode()).hexdigest()}.lock"
        with lock_path.open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result, writes, opened = self.probe({("titles",): "T2\tOriginal"})
        self.assertEqual(result, {})
        self.assertEqual(writes, [])
        opened.assert_not_called()
        self.ghostty.assert_not_called()
        # Once the owner finishes, a later hook can discover this surface.
        result, writes, _ = self.probe({("titles",): "T2\tOriginal", ("title", "marker"): "W2\nTB2\nT2"})
        self.assertEqual(result["terminal_id"], "T2")
        self.assertEqual(len(writes), 2)

    def test_failed_lookup_restores_title_without_repeated_timeouts(self):
        result, writes, _ = self.probe({("titles",): "T2\tOriginal"})
        self.assertEqual(result, {})
        self.assertEqual(len(writes), 2)
        self.assertEqual(writes[-1], "\x07")
        self.assertEqual(self.ghostty.call_count, 2)

    def test_unavailable_or_ambiguous_ghostty_falls_back_to_manual_pairing(self):
        origin.save_registration("55:/dev/ttys027", {"terminal_app": "com.mitchellh.ghostty", "window_id": "manual"})
        result, writes, _ = self.probe({("titles",): "T1\tx", ("title", "marker"): ""})
        self.assertEqual(result, {"window_id": "manual"})
        self.assertEqual(writes[-1], "\x07")  # the marker never outlives the probe
        self.ghostty.side_effect = None
        self.assertEqual(origin.probe_ghostty("/dev/ttys027"), {})  # scripting unavailable

    def test_probe_can_be_disabled_and_is_ghostty_only(self):
        with patch.object(origin, "process_table", return_value={}), patch.object(origin.sys, "platform", "darwin"), \
             patch.object(origin, "application", return_value={"terminal_app": "com.mitchellh.ghostty", "app_pid": 5}), \
             patch.object(origin, "probe_ghostty") as probe:
            origin.capture_origin({"TERM_PROGRAM": "ghostty", "NOTIFY_TTY": "/dev/ttys1", "NOTIFY_GHOSTTY_PROBE": "0"})
            self.assertEqual(origin.ghostty_origin({"terminal_app": "com.googlecode.iterm2", "app_pid": 5, "tty": "/dev/ttys1"}), {})
        probe.assert_not_called()

    def test_ancestry_stops_on_a_cycle(self):
        self.assertEqual(len(list(origin.ancestry({2: (3, "?", "sh"), 3: (2, "?", "sh")}, 2))), 2)


if __name__ == "__main__":
    unittest.main()
