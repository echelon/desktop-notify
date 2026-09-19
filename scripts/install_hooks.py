#!/usr/bin/env python3
"""Preview/install global hooks, preserving unrelated hooks and making backups.

The explicit --install option registers trust only for these exact definitions,
using hashes returned by the installed Codex version (never bypassing trust).
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


def configuration():
    document = json.loads(HOOKS_PATH.read_text()) if HOOKS_PATH.exists() else {"hooks": {}}
    hooks = document.setdefault("hooks", {})
    for name in ("Stop", "PermissionRequest", "PreToolUse"):
        groups = []
        for group in hooks.get(name, []):
            handlers = [h for h in group.get("hooks", []) if h.get("command") not in (OLD_COMMAND, COMMAND)]
            if handlers:
                groups.append({**group, "hooks": handlers})
        group = {"hooks": [{"type": "command", "command": COMMAND, "timeout": 720,
                            "statusMessage": "Notifying through Desktop Notify"}]}
        if name == "PreToolUse":
            group["matcher"] = MATCHER
        groups.append(group)
        hooks[name] = groups
    return document


def own_hooks(rpc):
    result = rpc.call("hooks/list", {"cwds": [str(ROOT)]})
    hooks = []
    for entry in result["data"]:
        if entry.get("errors"):
            raise RuntimeError(entry["errors"])
        hooks.extend(h for h in entry["hooks"]
                     if h["sourcePath"] == str(HOOKS_PATH) and h.get("command") == COMMAND)
    if len(hooks) != 3:
        raise RuntimeError(f"Expected 3 Desktop Notify hooks, got {len(hooks)}")
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
    args = parser.parse_args()
    if args.install:
        install()
    elif args.verify:
        verify()
    else:
        print(json.dumps(configuration(), indent=2))
