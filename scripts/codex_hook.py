#!/usr/bin/env python3
"""Codex command hook: consume stdin JSON, ensure server, POST an alert.

No shell interpolation of conversation data. No third-party Python dependencies.
"""
import fcntl
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request

from notification_origin import capture_origin

ROOT = Path(__file__).resolve().parents[1]
BASE_URL = "http://127.0.0.1:43110"
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def http(path, payload=None, timeout=2):
    data = None if payload is None else json.dumps(payload).encode()
    req = urllib.request.Request(BASE_URL + path, data=data,
                                 headers={"Content-Type": "application/json"} if data is not None else {})
    with OPENER.open(req, timeout=timeout) as response:
        return json.load(response)


def running():
    try:
        result = http("/health")
        return result.get("service") == "desktop-notify" and result.get("api_version") in (2, 3)
    except (OSError, ValueError):
        return False


def ensure_server():
    if running():
        ensure_desktop()
        return
    target = ROOT / "target"
    target.mkdir(exist_ok=True)
    with (target / "notify-start.lock").open("a") as lock:
        # Multiple Codex sessions may finish at once. Only one builds/starts.
        fcntl.flock(lock, fcntl.LOCK_EX)
        if running():
            ensure_desktop()
            return
        log_path = target / "desktop-notify.log"
        with log_path.open("ab", buffering=0) as log:
            subprocess.run([sys.executable, str(ROOT / "scripts/build.py")], cwd=ROOT,
                           stdout=log, stderr=log, check=True, timeout=600)
            env = os.environ.copy()
            env["HTTP_BIND_ADDRESS"] = "127.0.0.1:43110"
            process = subprocess.Popen([str(target / "debug/agent-notify-server")], cwd=ROOT,
                                       stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                       start_new_session=True, env=env)
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError(f"Server exited ({process.returncode}); see {log_path}")
            if running():
                return
            time.sleep(0.2)
        process.terminate()
        raise RuntimeError(f"Server startup timed out; see {log_path}")


def ensure_desktop():
    """Reopen the tray app if it was quit while the service remained running."""
    if sys.platform != "darwin" or os.environ.get("NOTIFY_DISABLE_DESKTOP"):
        return
    status = http("/state")
    if status.get("desktop_connected"):
        pid = status.get("desktop", {}).get("pid")
        if isinstance(pid, int) and pid > 0:
            try:
                os.kill(pid, 0)
                return
            except ProcessLookupError:
                pass
    app = ROOT / "target/desktop/Desktop Notify.app"
    if not app.exists():
        raise RuntimeError("Tauri app is missing; run python3 scripts/build.py")
    subprocess.run(["/usr/bin/open", "-g", str(app)], check=True, timeout=10,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def plain(text):
    text = re.sub(r"\[([^]]+)\]\([^)]+\)", r"\1", str(text))
    text = re.sub(r"(?m)^\s{0,3}#{1,6}\s+", "", text)
    return text.replace("**", "").replace("`", "").strip()


def clip(text, limit):
    text = plain(text)
    return text if len(text) <= limit else text[:limit - 1].rstrip() + "…"


def notification_for(event):
    name = event.get("hook_event_name")
    tool = event.get("tool_name", "")
    args = event.get("tool_input") or {}
    if isinstance(args, str):
        try:
            args = json.loads(args)
        except ValueError:
            args = {}
    cwd = Path(event.get("cwd") or "work").name
    if name == "Stop":
        message = event.get("last_assistant_message") or "The agent finished its turn."
        # Stop can be a plain-text question when the structured input tool is
        # unavailable. Keep those turns on the awaiting sound, not the done sound.
        awaiting = plain(message).endswith("?") or re.search(
            r"(?i)\b(waiting for your|awaiting your|need your (?:input|approval|confirmation|response))\b", message)
        endpoint = "/awaiting_user_input" if awaiting else "/all_tasks_finished"
    elif name == "PermissionRequest":
        description = args.get("description") or args.get("justification") or "Approval needed to continue"
        command = args.get("command") or args.get("cmd")
        message = description + (f"\n\n{command}" if isinstance(command, str) else "")
        endpoint = "/awaiting_user_input"
    elif name == "PreToolUse" and re.search(r"(?:request_user_input(?:_async)?|AskUserQuestion)$", tool):
        questions = args.get("questions") or []
        message = "\n\n".join(q.get("question") or q.get("title") or "" for q in questions if isinstance(q, dict))
        message = message or args.get("question") or "The agent needs your input to continue."
        endpoint = "/awaiting_user_input"
    else:
        return None
    first_line = next((line.strip() for line in plain(message).splitlines() if line.strip()), "Agent update")
    title = clip(event.get("title") or f"{cwd}: {first_line}", 120)
    payload = {"title": title, "message": clip(message, 4000)}
    # Codex supplies this on Stop, PermissionRequest and PreToolUse alike.
    # An explicit event ID wins over an inherited parent shell's thread ID.
    session_id = event.get("session_id") or os.environ.get("CODEX_THREAD_ID")
    if isinstance(session_id, str) and session_id.strip():
        payload["session_id"] = session_id
    return endpoint, payload


def main():
    try:
        event = json.load(sys.stdin)
        alert = notification_for(event)
        if alert:
            ensure_server()
            endpoint, payload = alert
            try:
                origin = capture_origin()
                if origin:
                    payload["origin"] = origin
            except Exception:
                # A missing terminal hint must never suppress the notification.
                pass
            result = http(endpoint, payload, timeout=5)
            if not result.get("id"):
                raise RuntimeError("Server did not acknowledge the alert")
        # A Stop hook must emit JSON, and no hook should change tool permissions.
        print("{}")
    except Exception as error:
        print(json.dumps({"systemMessage": f"Desktop Notify hook failed: {error}"}))


if __name__ == "__main__":
    main()
