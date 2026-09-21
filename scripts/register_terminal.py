#!/usr/bin/env python3
"""Run in a foreground Ghostty window to associate that window with its TTY."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid

import notification_origin as origins

SCRIPT = '''
tell application "Ghostty"
  if not frontmost then error "Bring the intended Ghostty window forward and run this command again."
  set w to front window
  set t to focused terminal of selected tab of w
  return (id of w as text) & linefeed & (id of t as text)
end tell
'''


def register(include_terminal=False):
    origin = origins.capture_origin(include_registered=False)
    if not origin or origin.get("terminal_app") != "com.mitchellh.ghostty":
        raise RuntimeError("Run this command from a shell in the Ghostty window you want to focus.")
    key = origins.registration_key(origin)
    if not key:
        raise RuntimeError("Could not identify this window's TTY and app process. If using tmux, attach its client first.")
    result = subprocess.run(["/usr/bin/osascript", "-e", SCRIPT], capture_output=True,
                            text=True, timeout=60)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "Could not read the foreground Ghostty window.")
    ids = result.stdout.strip().splitlines()
    if len(ids) != 2 or any(not value.strip() or len(value) > 1024 for value in ids):
        raise RuntimeError("Ghostty did not return a window and terminal ID.")
    entry = {"terminal_app": "com.mitchellh.ghostty", "window_id": ids[0]}
    if include_terminal:
        entry["terminal_id"] = ids[1]
    path = origins.REGISTRATIONS
    path.parent.mkdir(exist_ok=True)
    with path.with_suffix(".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        entries = json.loads(path.read_text()) if path.exists() else {}
        entries[key] = entry
        temporary = path.with_suffix(f".{os.getpid()}.tmp")
        try:
            temporary.write_text(json.dumps(entries, indent=2) + "\n")
            temporary.chmod(0o600)
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
    # Isolate window focus during pairing; don't switch any tmux panes.
    return {"terminal_app": entry["terminal_app"], "app_pid": origin["app_pid"],
            "window_id": entry["window_id"], **({"terminal_id": ids[1]} if include_terminal else {})}


def test_alert(origin, delay):
    import codex_hook as hook
    if not hook.running():
        raise RuntimeError("Start the notification server before running --test.")
    print(f"Switch to another Space now. A test alert will appear in {delay} seconds.", flush=True)
    time.sleep(delay)
    notification = hook.http("/awaiting_user_input", {
        "session_id": "focus-test-" + uuid.uuid4().hex,
        "title": "Focus test: this Ghostty window",
        "message": "Click Focus. It should return to the window where you ran register_terminal.py, including its Space. The test clears itself after 60 seconds.",
        "origin": origin,
    })
    print(f'Test notification: {notification["id"]}', flush=True)
    report = {"notification": notification, "samples": []}
    try:
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            state = hook.http("/state")
            if not any(n["id"] == notification["id"] for n in state.get("notifications", [])):
                break
            sample = state.get("desktop", {})
            if not report["samples"] or report["samples"][-1] != sample:
                report["samples"].append(sample)
            time.sleep(.5)
    finally:
        # An older test must never dismiss a replacement alert.
        hook.http("/dismiss/" + notification["id"], {})
        path = origins.REGISTRATIONS.parent / "focus-pair-test.json"
        path.write_text(json.dumps(report, indent=2) + "\n")
        print(f"Test finished; sound stopped. Report: {path}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--terminal", action="store_true", help="Also register the selected Ghostty terminal/pane; default registers just the window")
    parser.add_argument("--test", action="store_true", help="Send a delayed test notification, with automatic cleanup")
    parser.add_argument("--delay", type=int, default=10, help="Seconds before the test alert (default: 10)")
    args = parser.parse_args()
    if not 0 <= args.delay <= 120:
        parser.error("--delay must be between 0 and 120 seconds")
    try:
        origin = register(args.terminal)
        print("Registered:", json.dumps(origin), flush=True)
        if args.test:
            test_alert(origin, args.delay)
    except (RuntimeError, OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"Registration failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
