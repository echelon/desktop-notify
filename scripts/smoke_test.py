#!/usr/bin/env python3
"""Exercise the installed global hook commands against the real local server.

--restart also tests a cold build/start with simultaneous hook invocations.
Sound plays briefly during the test and is always stopped afterward.
"""
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import time
import urllib.request

import codex_hook as hook
from install_hooks import HOOKS_PATH, COMMAND


def eventually(predicate, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(0.2)
    raise AssertionError("Timed out waiting for expected state")


def invoke(event):
    config = json.loads(HOOKS_PATH.read_text())
    handlers = [h for group in config["hooks"][event["hook_event_name"]]
                for h in group["hooks"] if h.get("command") == COMMAND]
    assert len(handlers) == 1
    result = subprocess.run(shlex.split(handlers[0]["command"]), input=json.dumps(event),
                            cwd="/tmp", capture_output=True, text=True, timeout=720, check=True)
    assert json.loads(result.stdout) == {}, result.stdout + result.stderr


def stop():
    req = urllib.request.Request(hook.BASE_URL + "/stop", data=b"", method="POST")
    with hook.OPENER.open(req, timeout=3) as response:
        assert response.status == 200


def run(restart=False):
    previous_pid = None
    if restart and hook.running():
        previous_pid = hook.http("/health")["pid"]
        os.kill(previous_pid, signal.SIGTERM)
        eventually(lambda: not hook.running())
    try:
        done = {"hook_event_name": "Stop", "cwd": "/tmp/hook-smoke-test",
                "last_assistant_message": "Desktop Notify completion hook test passed."}
        # Both start from a stopped server with --restart. The lock must prevent
        # duplicate listeners and build races. Both run the installed command.
        with ThreadPoolExecutor(max_workers=2) as pool:
            list(pool.map(invoke, [done, done]))
        pid = hook.http("/health")["pid"]
        if previous_pid:
            assert pid != previous_pid
        eventually(lambda: hook.http("/state")["audio"]["loop_name"] == "done")
        old = hook.http("/notification")
        assert old["kind"] == "all_tasks_finished"
        print(f"PASS Stop -> done loop; server pid={pid}", flush=True)

        for event in [
            {"hook_event_name": "PreToolUse", "tool_name": "request_user_input",
             "tool_input": {"questions": [{"question": "Does this Tauri notification stay visible?"}]}},
            {"hook_event_name": "PreToolUse", "tool_name": "request_user_input_async",
             "tool_input": {"questions": [{"title": "Can you see Dismiss & Stop?"}]}},
            {"hook_event_name": "PermissionRequest", "tool_input": {"description": "Test approval request"}},
        ]:
            invoke({**event, "cwd": "/tmp/hook-smoke-test"})
            current = hook.http("/notification")
            assert current["kind"] == "awaiting_user_input"
            eventually(lambda: hook.http("/state")["audio"]["loop_name"] == "await")
            assert hook.http("/health")["pid"] == pid, "Warm hook started another server"
            print(f'PASS {event["hook_event_name"]}/{event.get("tool_name", "approval")} -> await loop', flush=True)

        assert hook.http("/dismiss/" + old["id"], {}) == {"stopped": False}
        assert hook.http("/notification")["id"] == current["id"]
        assert hook.http("/dismiss/" + current["id"], {}) == {"stopped": True}
        eventually(lambda: not hook.http("/state")["audio"]["loop_playing"])
        assert hook.http("/notification") is None
        print("PASS stale dismissal preserved newer alert; current dismissal stopped sound", flush=True)
        eventually(lambda: hook.http("/state")["desktop_connected"])
        print("Desktop status:", json.dumps(hook.http("/state")["desktop"]), flush=True)
    finally:
        if hook.running():
            stop()


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--restart", action="store_true")
    parser.add_argument("--show-demo", action="store_true", help="Leave a visible alert for a manual dismiss-button test")
    args = parser.parse_args()
    run(args.restart)
    if args.show_demo:
        hook.http("/awaiting_user_input", {
            "title": "Your new notification tray app",
            "message": "Alerts now use Rust and Tauri, like Todo.\n\nHide this window with the minus button, then click the bell in your menu bar to bring it back.\n\nDismiss & Stop clears the alert and silences the sound."
        })
