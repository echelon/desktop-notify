import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import MagicMock, patch

import install_hooks


class CodexInstallTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.directory = Path(temp.name) / "codex"
        for name, value in (("CODEX_DIR", self.directory), ("HOOKS_PATH", self.directory / "hooks.json"),
                            ("CONFIG_PATH", self.directory / "config.toml"), ("GUIDANCE_PATH", self.directory / "AGENTS.md")):
            patch.object(install_hooks, name, value).start()
        self.addCleanup(patch.stopall)

    def test_replaces_older_definitions_and_preserves_unrelated_hooks(self):
        self.directory.mkdir()
        unrelated = {"matcher": "Bash", "hooks": [{"type": "command", "command": "custom-check"}]}
        legacy = {"hooks": [{"type": "command", "command": install_hooks.OLD_COMMAND}]}
        old_script = {"hooks": [{"type": "command", "command": "python3 " + install_hooks.HOOK_SCRIPT}]}
        original = {"extra": "kept", "hooks": {"Stop": [legacy, old_script, unrelated], "SessionStart": [unrelated]}}
        install_hooks.HOOKS_PATH.write_text(json.dumps(original))
        updated = install_hooks.configuration()
        self.assertEqual(updated["extra"], "kept")
        self.assertEqual(updated["hooks"]["SessionStart"], [unrelated])
        self.assertEqual(updated["hooks"]["Stop"][0], unrelated)
        for event, matcher, timeout, status in install_hooks.EVENTS:
            group = updated["hooks"][event][-1]
            self.assertEqual(group.get("matcher"), matcher)
            self.assertEqual(group["hooks"][0]["command"], install_hooks.COMMAND)
            self.assertEqual(group["hooks"][0]["timeout"], timeout)
            self.assertNotIn("async", group["hooks"][0])
        install_hooks.HOOKS_PATH.write_text(json.dumps(updated))
        self.assertEqual(install_hooks.configuration(), updated)

    def test_guidance_replaces_its_section_and_preserves_personal_rules(self):
        self.directory.mkdir()
        before, after = "# Personal rules\nKeep this.\n\n", "\n\n# More rules\nKeep these too.\n"
        install_hooks.GUIDANCE_PATH.write_text(before + install_hooks.GUIDANCE_START + "\nOld guidance\n"
                                              + install_hooks.GUIDANCE_END + after)
        updated = install_hooks.guidance_configuration()
        self.assertEqual(updated, before + install_hooks.guidance_block().rstrip("\n") + after)
        self.assertNotIn("@DESKTOP_NOTIFY_ROOT@", updated)
        install_hooks.GUIDANCE_PATH.write_text(updated)
        self.assertEqual(install_hooks.guidance_configuration(), updated)

    def test_malformed_markers_and_existing_claude_file_are_not_overwritten(self):
        self.directory.mkdir()
        for source in (install_hooks.GUIDANCE_START, install_hooks.GUIDANCE_END,
                       install_hooks.GUIDANCE_END + install_hooks.GUIDANCE_START,
                       install_hooks.guidance_block() * 2):
            install_hooks.GUIDANCE_PATH.write_text(source)
            with self.assertRaises(RuntimeError):
                install_hooks.install(with_guidance=True)
            self.assertEqual(install_hooks.GUIDANCE_PATH.read_text(), source)
            self.assertFalse(install_hooks.HOOKS_PATH.exists())
        install_hooks.GUIDANCE_PATH.write_text("Personal rules")
        (self.directory / "CLAUDE.md").write_text("Separate existing rules")
        with self.assertRaises(RuntimeError):
            install_hooks.install(with_guidance=True)
        self.assertFalse(install_hooks.HOOKS_PATH.exists())

    def test_install_creates_home_and_trusts_only_returned_hook_hashes(self):
        rpc = MagicMock()
        hooks = [{"sourcePath": str(install_hooks.HOOKS_PATH), "command": install_hooks.COMMAND,
                  "eventName": event, "key": f"hook-{event}", "currentHash": f"hash-{event}",
                  "enabled": True, "trustStatus": "trusted"} for event, *_ in install_hooks.EVENTS]
        rpc.call.return_value = {"data": [{"hooks": hooks}]}
        with patch.object(install_hooks, "CodexRPC") as client:
            client.return_value.__enter__.return_value = rpc
            install_hooks.install(with_guidance=True)
            self.assertEqual(install_hooks.GUIDANCE_PATH.read_text(), install_hooks.guidance_block())
            self.assertEqual((self.directory / "CLAUDE.md").readlink(), Path("AGENTS.md"))
            # Reinstall preserves an unrelated notify integration and backs it up.
            notify = 'notify = ["computer-use-client", "turn-ended"]\n'
            install_hooks.CONFIG_PATH.write_text(notify)
            install_hooks.install(with_guidance=True)
        self.assertEqual(install_hooks.CONFIG_PATH.read_text(), notify)
        backups = list(self.directory.glob("config.toml.desktop-notify-backup-*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_text(), notify)
        edits = next(call.args[1]["edits"] for call in rpc.call.call_args_list if call.args[0] == "config/batchWrite")
        hashes = [e["value"] for e in edits if e["keyPath"].endswith(".trusted_hash")]
        self.assertEqual(hashes, [h["currentHash"] for h in hooks])
        self.assertFalse(any(e["keyPath"] == "notify" for e in edits))


class ClaudeInstallTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.settings = Path(temp.name) / "settings.json"
        patch.object(install_hooks, "CLAUDE_SETTINGS", self.settings).start()
        self.addCleanup(patch.stopall)

    def test_replaces_legacy_hooks_and_preserves_unrelated_settings(self):
        legacy = lambda arg: {"hooks": [{"type": "command", "command": f"bash ~/.claude/agent_notify.sh {arg}", "async": True}]}
        unrelated = {"matcher": "Write|Edit", "hooks": [{"type": "command", "command": "prettier --write"}]}
        self.settings.write_text(json.dumps({"theme": "dark", "hooks": {
            "Stop": [legacy("done")],
            "UserPromptSubmit": [legacy("stop")],
            "PostToolUse": [{"matcher": "*", **legacy("stop")}, unrelated],
            "Notification": [{"hooks": [{"type": "command", "command": "bash ~/.claude/agent_notify_on_notification.sh"}]}],
        }}))
        install_hooks.install_claude()
        document = json.loads(self.settings.read_text())
        hooks = document["hooks"]
        self.assertEqual(document["theme"], "dark")
        # The legacy /stop hook is gone; unrelated hooks stay alongside ours.
        self.assertEqual(hooks["PostToolUse"][0], unrelated)
        self.assertEqual(len(hooks["PostToolUse"]), 2)
        self.assertNotIn("Notification", hooks)
        for name, _, timeout, _ in install_hooks.CLAUDE_EVENTS:
            group = hooks[name][-1]
            self.assertEqual(group["hooks"][0]["command"], install_hooks.CLAUDE_COMMAND)
            self.assertTrue(group["hooks"][0]["command"].endswith("--agent claude_code"))
            self.assertEqual(group["hooks"][0]["timeout"], timeout)
        self.assertNotIn("statusMessage", hooks["PostToolUse"][-1]["hooks"][0])
        self.assertEqual(hooks["PreToolUse"][0]["matcher"], install_hooks.MATCHER)
        self.assertEqual(len(list(self.settings.parent.glob("settings.json.desktop-notify-backup-*"))), 1)
        # Reinstalling replaces rather than duplicates this hook.
        install_hooks.install_claude()
        self.assertEqual(json.loads(self.settings.read_text())["hooks"], hooks)

    def test_creates_settings_when_absent(self):
        install_hooks.install_claude()
        self.assertEqual(set(json.loads(self.settings.read_text())["hooks"]),
                         {name for name, *_ in install_hooks.CLAUDE_EVENTS})


if __name__ == "__main__":
    unittest.main()
