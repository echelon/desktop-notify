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
import uuid

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


def session_notification(session_id):
    return next((n for n in hook.http("/notifications") if n.get("session_id") == session_id), None)


def clear_sessions(session_ids):
    if hook.running():
        for notification in hook.http("/notifications"):
            if notification.get("session_id") in session_ids:
                hook.http("/dismiss/" + notification["id"], {})


def run(restart=False):
    previous_pid = None
    if restart and hook.running():
        previous_pid = hook.http("/health")["pid"]
        os.kill(previous_pid, signal.SIGTERM)
        eventually(lambda: not hook.running())
    sessions = ["smoke-" + uuid.uuid4().hex for _ in range(2)]
    try:
        done = {"hook_event_name": "Stop", "cwd": "/tmp/hook-smoke-test",
                "last_assistant_message": "Desktop Notify completion hook test passed."}
        # Independent sessions also exercise the shared cold-start lock.
        with ThreadPoolExecutor(max_workers=2) as pool:
            list(pool.map(invoke, [{**done, "session_id": session} for session in sessions]))
        health = hook.http("/health")
        pid = health["pid"]
        assert health["api_version"] == 3
        if previous_pid:
            assert pid != previous_pid
        a, b = [session_notification(session) for session in sessions]
        assert a and b and a["id"] != b["id"]
        assert a["kind"] == b["kind"] == "all_tasks_finished"
        if hook.sys.platform == "darwin":
            assert a.get("origin", {}).get("pid"), "Installed hook did not attach focus metadata"
        print("PASS concurrent installed hooks created two independent session rows", flush=True)

        for event in [
            {"hook_event_name": "PreToolUse", "tool_name": "request_user_input",
             "tool_input": {"questions": [{"question": "Does this notification stay visible?"}]}},
            {"hook_event_name": "PreToolUse", "tool_name": "request_user_input_async",
             "tool_input": {"questions": [{"title": "Can you see both agent rows?"}]}},
            {"hook_event_name": "PermissionRequest", "tool_input": {"description": "Test approval request"}},
        ]:
            invoke({**event, "cwd": "/tmp/hook-smoke-test", "session_id": sessions[0]})
            current = session_notification(sessions[0])
            assert current["kind"] == "awaiting_user_input"
            assert session_notification(sessions[1]) == b
            eventually(lambda: hook.http("/state")["audio"]["loop_name"] == "await")
            assert hook.http("/health")["pid"] == pid
        print("PASS questions and approvals update only their own session", flush=True)

        assert hook.http("/dismiss/" + a["id"], {}) == {"stopped": False}
        assert hook.http("/silence/" + a["id"], {}) == {"stopped": False}
        assert session_notification(sessions[0])["id"] == current["id"]
        assert hook.http("/silence/" + current["id"], {}) == {"stopped": True}
        assert session_notification(sessions[0])["silenced"]
        assert session_notification(sessions[1]) == b
        assert hook.http("/dismiss/" + b["id"], {}) == {"stopped": True}
        assert session_notification(sessions[0])["id"] == current["id"]
        assert session_notification(sessions[1]) is None
        print("PASS stale actions are harmless; silence retains status; clear removes only its row", flush=True)

        # A later completion on A reuses its row and re-arms its sound.
        invoke({**done, "session_id": sessions[0]})
        updated = session_notification(sessions[0])
        assert updated["kind"] == "all_tasks_finished" and not updated["silenced"]
        assert updated["id"] != current["id"]
        eventually(lambda: updated["id"] in hook.http("/state")["desktop"].get("displayed_ids", []))
        print("PASS new update re-arms the session and reaches the running desktop app", flush=True)
    finally:
        clear_sessions(sessions)


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--restart", action="store_true")
    parser.add_argument("--show-demo", action="store_true", help="Leave two visible sample agent rows")
    args = parser.parse_args()
    run(args.restart)
    if args.show_demo:
        for session, endpoint, title, message in [
            ("demo-question", "/awaiting_user_input", "Agent A: input needed", "Each session has its own Focus, Stop sound, and × buttons."),
            ("demo-finished", "/all_tasks_finished", "Agent B: work completed", "Clearing one row leaves the other agent’s status intact."),
        ]:
            hook.http(endpoint, {"session_id": session, "title": title, "message": message})
