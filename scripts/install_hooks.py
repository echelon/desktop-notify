#!/usr/bin/env python3
"""Preview/install global hooks, preserving unrelated hooks and making backups.

The explicit --install option registers trust only for these exact definitions,
using hashes returned by the installed Codex version (never bypassing trust).
--claude targets Claude Code's settings.json instead; it has no trust registry.
"""
import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import shlex
import shutil
import sys

from codex_rpc import CodexRPC

ROOT = Path(__file__).resolve().parents[1]
CODEX_DIR = Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))
HOOKS_PATH = CODEX_DIR / "hooks.json"
CONFIG_PATH = CODEX_DIR / "config.toml"
COMMAND = shlex.join([sys.executable, str(ROOT / "scripts/codex_hook.py")])
OLD_COMMAND = "for i in 1 2 3; do afplay /Users/bt/dev/storyteller/artcraft/frontend/apps/artcraft/app/public/resources/sound/smrpg_flower.wav ; sleep 0.5; done &"
MATCHER = r"(^|.*[._])(request_user_input(_async)?|AskUserQuestion)$"
STATUS = "Notifying through Desktop Notify"
# (event, matcher, timeout seconds, status message). Alerts may cold-build and
# start the service, hence 720 s. Working updates never start it, so they get
# short limits and no status line (PostToolUse runs after every tool). All are
# synchronous: an asynchronous PostToolUse could land after a newer question.
EVENTS = [
    ("Stop", None, 720, STATUS),
    ("PermissionRequest", None, 720, STATUS),
    ("PreToolUse", MATCHER, 720, STATUS),
    ("UserPromptSubmit", None, 30, None),
    ("PostToolUse", None, 10, None),
]
# Claude Code also reports turns that end on an API error.
CLAUDE_EVENTS = EVENTS + [("StopFailure", None, 720, STATUS)]
CLAUDE_SETTINGS = Path(os.environ.get("CLAUDE_CONFIG_DIR", Path.home() / ".claude")) / "settings.json"
# Earlier Claude Code wiring called legacy sound-only endpoints: /loop_* and /stop
# clear every session's row, so they must not run alongside the row-based hook.
LEGACY_CLAUDE_SCRIPTS = ("agent_notify.sh", "agent_notify_on_notification.sh")


def hook_group(matcher, timeout, status):
    handler = {"type": "command", "command": COMMAND, "timeout": timeout}
    if status:
        handler["statusMessage"] = status
    return {"hooks": [handler], **({"matcher": matcher} if matcher else {})}


def configuration():
    document = json.loads(HOOKS_PATH.read_text()) if HOOKS_PATH.exists() else {"hooks": {}}
    hooks = document.setdefault("hooks", {})
    for name, matcher, timeout, status in EVENTS:
        groups = []
        for group in hooks.get(name, []):
            handlers = [h for h in group.get("hooks", []) if h.get("command") not in (OLD_COMMAND, COMMAND)]
            if handlers:
                groups.append({**group, "hooks": handlers})
        groups.append(hook_group(matcher, timeout, status))
        hooks[name] = groups
    return document


def claude_configuration():
    """Same events, command, and matcher as Codex, in Claude Code's settings.json."""
    document = json.loads(CLAUDE_SETTINGS.read_text()) if CLAUDE_SETTINGS.exists() else {}
    hooks = document.setdefault("hooks", {})
    for name in list(hooks):
        groups = []
        for group in hooks[name]:
            handlers = [h for h in group.get("hooks", [])
                        # Any interpreter running this repo's hook counts as ours.
                        if not h.get("command", "").endswith(str(ROOT / "scripts/codex_hook.py"))
                        and not any(script in h.get("command", "") for script in LEGACY_CLAUDE_SCRIPTS)]
            if handlers:
                groups.append({**group, "hooks": handlers})
        if groups:
            hooks[name] = groups
        else:
            del hooks[name]
    for name, matcher, timeout, status in CLAUDE_EVENTS:
        hooks.setdefault(name, []).append(hook_group(matcher, timeout, status))
    return document


def install_claude():
    document = claude_configuration()
    if CLAUDE_SETTINGS.exists():
        backup = CLAUDE_SETTINGS.with_name(CLAUDE_SETTINGS.name + ".desktop-notify-backup-"
                                           + datetime.now().strftime("%Y%m%d-%H%M%S-%f"))
        shutil.copy2(CLAUDE_SETTINGS, backup)
        print(f"Backup: {backup}")
    CLAUDE_SETTINGS.parent.mkdir(parents=True, exist_ok=True)
    temporary = CLAUDE_SETTINGS.with_suffix(".json.desktop-notify-tmp")
    temporary.write_text(json.dumps(document, indent=2) + "\n")
    temporary.replace(CLAUDE_SETTINGS)
    print(f"Claude Code hooks: {CLAUDE_SETTINGS}")


def own_hooks(rpc):
    result = rpc.call("hooks/list", {"cwds": [str(ROOT)]})
    hooks = []
    for entry in result["data"]:
        if entry.get("errors"):
            raise RuntimeError(entry["errors"])
        hooks.extend(h for h in entry["hooks"]
                     if h["sourcePath"] == str(HOOKS_PATH) and h.get("command") == COMMAND)
    if len(hooks) != len(EVENTS):
        raise RuntimeError(f"Expected {len(EVENTS)} Desktop Notify hooks, got {len(hooks)}")
    return hooks


def install():
    document = configuration()
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    for path in (HOOKS_PATH, CONFIG_PATH):
        if path.exists():
            backup = path.with_name(path.name + ".desktop-notify-backup-" + stamp)
            shutil.copy2(path, backup)
            print(f"Backup: {backup}")
    temporary = HOOKS_PATH.with_suffix(".json.desktop-notify-tmp")
    temporary.write_text(json.dumps(document, indent=2) + "\n")
    temporary.replace(HOOKS_PATH)
    with CodexRPC() as rpc:
        hooks = own_hooks(rpc)
        edits = [{"keyPath": f'hooks.state.{json.dumps(h["key"])}.trusted_hash',
                  "value": h["currentHash"], "mergeStrategy": "replace"} for h in hooks]
        edits += [{"keyPath": f'hooks.state.{json.dumps(h["key"])}.enabled',
                   "value": True, "mergeStrategy": "replace"} for h in hooks]
        edits.append({"keyPath": "features.hooks", "value": True, "mergeStrategy": "replace"})
        rpc.call("config/batchWrite", {"edits": edits, "filePath": str(CONFIG_PATH)})
    verify()


def verify():
    with CodexRPC() as rpc:
        for hook in own_hooks(rpc):
            print(f'{hook["eventName"]}: enabled={hook["enabled"]}, trust={hook["trustStatus"]}, command={hook["command"]}')
            if not hook["enabled"] or hook["trustStatus"] != "trusted":
                raise RuntimeError("Hook is not enabled and trusted")
    print(f"Global hooks: {HOOKS_PATH}")
    print(f"Hook trust: {CONFIG_PATH}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--install", action="store_true")
    mode.add_argument("--verify", action="store_true")
    parser.add_argument("--claude", action="store_true", help="target Claude Code instead of Codex")
    args = parser.parse_args()
    if args.claude:
        if args.verify:
            parser.error("--verify applies to Codex hook trust only")
        install_claude() if args.install else print(json.dumps(claude_configuration(), indent=2))
    elif args.install:
        install()
    elif args.verify:
        verify()
    else:
        print(json.dumps(configuration(), indent=2))
