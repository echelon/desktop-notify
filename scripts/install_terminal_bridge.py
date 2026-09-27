#!/usr/bin/env python3
"""Install a shell function that gives local Codex TUI launches session origins.

The user's shared shell loader sources ~/.config/shell/aliases/*.sh. Other
setups can source the generated file explicitly. Preview is the default.
"""
import argparse
from datetime import datetime
from pathlib import Path
import shlex
import sys

ROOT = Path(__file__).resolve().parents[1]


def definition():
    return ("# Desktop Notify: bind terminal Codex sessions to the shared daemon.\n"
            "# Utilities and explicit remote hosts pass through unchanged.\n"
            "codex() {\n  command " + shlex.quote(sys.executable) + " "
            + shlex.quote(str(ROOT / "scripts/codex_terminal.py")) + ' "$@"\n}\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--install", action="store_true")
    parser.add_argument("--verify", action="store_true")
    parser.add_argument("--path", type=Path, default=Path.home() / ".config/shell/aliases/40-codex-notify.sh")
    args = parser.parse_args()
    text = definition()
    if args.verify:
        if not args.path.exists() or args.path.read_text() != text:
            raise SystemExit("Terminal bridge definition does not match: " + str(args.path))
        print("Terminal bridge verified:", args.path)
    elif args.install:
        args.path.parent.mkdir(parents=True, exist_ok=True)
        if args.path.exists() and args.path.read_text() != text:
            backup = args.path.with_name(args.path.name + ".backup-" + datetime.now().strftime("%Y%m%d-%H%M%S-%f"))
            backup.write_bytes(args.path.read_bytes())
            print("Backup:", backup)
        args.path.write_text(text)
        print("Installed:", args.path)
        print("Load in existing terminal shells: source " + shlex.quote(str(args.path)))
    else:
        print("Would install:", args.path)
        print(text, end="")


if __name__ == "__main__":
    main()
