import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import install_hooks


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
