import io
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

import codex_terminal as terminal
import notification_origin as origin
import session_origin


class SessionOriginTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        patch.object(session_origin, "DIRECTORY", Path(directory.name)).start()
        patch.object(session_origin, "process_start", return_value="started today").start()
        patch.object(origin, "ghostty", return_value=None).start()
        self.addCleanup(patch.stopall)

    def test_two_sessions_same_directory_have_distinct_terminal_clients(self):
        for session, pid, pane in (("a", 42, "%5"), ("b", 43, "%6")):
            session_origin.save(session, pid, "started today", {"TMUX_PANE": pane, "SECRET": "excluded"})
        processes = {42: (1, "ttys001", "codex"), 43: (1, "ttys002", "codex")}
        self.assertEqual(session_origin.load("a", processes), (42, {"TMUX_PANE": "%5"}))
        self.assertEqual(session_origin.load("b", processes), (43, {"TMUX_PANE": "%6"}))

    def test_stale_missing_and_reused_processes_never_resolve(self):
        session_origin.save("a", 42, "yesterday", {})
        for processes in ({}, {42: (1, "ttys001", "codex")}, {42: (1, "ttys001", "other")},
                          {42: (1, "??", "codex")}):
            self.assertIsNone(session_origin.load("a", processes))
        self.assertIsNone(session_origin.load("missing", {}))

    def test_binding_replaces_daemon_environment_and_ancestry(self):
        session_origin.save("a", 42, "started today", {"TERM_PROGRAM": "ghostty"})
        processes = {42: (1, "ttys001", "codex"), 99: (1, "??", "codex")}
        with patch.object(origin, "process_table", return_value=processes), \
             patch.object(origin.os, "getppid", return_value=99), patch.object(origin.sys, "platform", "darwin"), \
             patch.object(origin, "application", return_value={}):
            result = origin.capture_origin({"TMUX": "/wrong,1,0", "TMUX_PANE": "%139"}, session_id="a")
        self.assertEqual(result["pid"], 42)
        self.assertEqual(result["tty"], "/dev/ttys001")
        self.assertNotIn("tmux_pane", result)

    def test_malformed_registration_is_optional(self):
        session_origin.binding_path("a").write_text('[]')
        self.assertIsNone(session_origin.load("a", {}))
        self.assertIsNone(session_origin.load("../escape\n", {}))


class BridgeTests(unittest.TestCase):
    def setUp(self):
        self.save = patch.object(session_origin, "save").start()
        self.addCleanup(patch.stopall)
        self.bindings = terminal.Bindings(42, "today", {"TMUX_PANE": "%5"})

    def test_start_resume_fork_and_turns_bind_exact_session_ids(self):
        for i, method in enumerate(("thread/start", "thread/resume", "thread/fork")):
            self.bindings.observe({"id": i, "method": method}, True)
            self.bindings.observe({"id": i, "result": {"thread": {"id": str(i)}}}, False)
            self.assertEqual(self.save.call_args.args[0], str(i))
        for method in ("turn/start", "turn/steer", "review/start"):
            self.bindings.observe({"id": "turn", "method": method, "params": {"threadId": "other"}}, True)
            self.assertEqual(self.save.call_args.args[0], "other")

    def test_reading_other_sessions_and_failed_resumes_do_not_steal_binding(self):
        self.bindings.observe({"id": 1, "method": "thread/read", "params": {"threadId": "other"}}, True)
        self.bindings.observe({"id": 1, "result": {"thread": {"id": "other"}}}, False)
        self.bindings.observe({"id": 2, "method": "thread/resume"}, True)
        self.bindings.observe({"id": 2, "error": {"message": "missing"}}, False)
        self.save.assert_not_called()

    def frame(self, data, opcode=1, final=True, masked=True):
        mask = b"abcd" if masked else b""
        n = len(data)
        extra = b"" if n < 126 else struct.pack("!H", n) if n < 65536 else struct.pack("!Q", n)
        header = bytes([(128 if final else 0) | opcode,
                        (128 if masked else 0) | (n if n < 126 else 126 if n < 65536 else 127)])
        encoded = bytes(c ^ mask[i % 4] for i, c in enumerate(data)) if mask else data
        return header + extra + mask + encoded

    def test_masked_fragmented_frames_forward_unchanged_and_bind_before_final_frame(self):
        payload = json.dumps({"id": 1, "method": "turn/start", "params": {"threadId": "abc"}}).encode()
        first = self.frame(payload[:20], final=False)
        ping = self.frame(b"ping", opcode=9)
        last = self.frame(payload[20:], opcode=0)
        sent = bytearray()
        test = self
        class Destination:
            def sendall(self, data):
                if data == last:
                    test.save.assert_called_once_with("abc", 42, "today", {"TMUX_PANE": "%5"})
                sent.extend(data)
        with self.assertRaises(EOFError):
            terminal.relay(io.BytesIO(first + ping + last), Destination(), self.bindings, True)
        self.assertEqual(sent, first + ping + last)

    def test_extended_lengths_and_binary_frames_are_preserved(self):
        frames = self.frame(b"x" * 200, opcode=2) + self.frame(b"x" * 70000, opcode=2)
        sent = bytearray()
        class Destination:
            def sendall(self, data): sent.extend(data)
        with self.assertRaises(EOFError):
            terminal.relay(io.BytesIO(frames), Destination(), self.bindings, True)
        self.assertEqual(sent, frames)
        self.save.assert_not_called()

    def test_utility_and_explicit_remote_commands_pass_through(self):
        for args in ([], ["agents"], ["resume", "abc"], ["fork"], ["-c", 'model="x"', "hello"], ["-C", "/repo"]):
            self.assertTrue(terminal.interactive(args), args)
        for args in (["exec", "hello"], ["app-server", "daemon", "start"], ["--remote", "unix://"],
                     ["--no-daemon"], ["--help"], ["resume", "--remote", "unix://"],
                     ["--worktree"], ["--profile", "custom"], ["-c", "x=true", "features", "list"]):
            self.assertFalse(terminal.interactive(args), args)

    def test_reconnect_command_gets_a_fresh_terminal_binding(self):
        self.assertEqual(terminal.without_old_bridge(["--remote", "unix:///tmp/dn-abc/codex.sock", "resume", "id"]),
                         ["resume", "id"])
        self.assertEqual(terminal.without_old_bridge(["--remote=unix:///tmp/dn-abc/codex.sock", "agents"]), ["agents"])
        args = ["--remote", "unix:///other/server.sock", "resume", "id"]
        self.assertEqual(terminal.without_old_bridge(args), args)

    def test_unavailable_registration_storage_does_not_block_codex(self):
        self.save.side_effect = OSError("read-only")
        self.bindings.observe({"id": 1, "method": "turn/start", "params": {"threadId": "a"}}, True)


if __name__ == "__main__":
    unittest.main()
