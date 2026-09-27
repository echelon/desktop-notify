#!/usr/bin/env python3
"""Codex/Claude Code command hook: consume stdin JSON, ensure server, POST an alert.

No shell interpolation of conversation data. No third-party Python dependencies.
"""
from datetime import datetime, timezone
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
from notification_context import capture_context

ROOT = Path(__file__).resolve().parents[1]
# Rolling record of hook decisions for after-the-fact debugging ("why did this
# row say input needed?"). Trimmed to its newest half past the limit.
EVENT_LOG = ROOT / "target/hook-events.jsonl"
EVENT_LOG_LIMIT = 512 * 1024
SERVICE_LOG_LIMIT = 5 * 1024 * 1024
BASE_URL = "http://127.0.0.1:43110"
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def http(path, payload=None, timeout=2):
    data = None if payload is None else json.dumps(payload).encode()
    req = urllib.request.Request(BASE_URL + path, data=data,
                                 headers={"Content-Type": "application/json"} if data is not None else {})
    with OPENER.open(req, timeout=timeout) as response:
        return json.load(response)


# API 4 added task states (/working, /task_failed). Older services still take
# questions and completions, so hooks degrade instead of failing.
TASK_STATE_API = 4


def service_version():
    try:
        result = http("/health")
    except (OSError, ValueError):
        return None
    if result.get("service") == "desktop-notify" and result.get("api_version") in (2, 3, 4):
        return result["api_version"]
    return None


def running():
    return service_version() is not None


def compatible(endpoint, payload):
    """An older service has no failed state; report the failure as finished."""
    if endpoint == "/task_failed" and (service_version() or 0) < TASK_STATE_API:
        return "/all_tasks_finished", payload
    return endpoint, payload


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
        # Keep one previous log rather than letting it grow without bound.
        if log_path.exists() and log_path.stat().st_size > SERVICE_LOG_LIMIT:
            log_path.replace(log_path.with_name(log_path.name + ".1"))
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


def last_transcript_text(path, limit=1024 * 1024):
    """Claude Code fallback when Stop omits last_assistant_message: the newest
    assistant text in its JSONL transcript. Unreadable/unknown shapes yield None."""
    try:
        with open(path, "rb") as transcript:
            transcript.seek(0, os.SEEK_END)
            transcript.seek(max(0, transcript.tell() - limit))
            lines = transcript.read().decode("utf-8", "replace").splitlines()
    except (OSError, TypeError, ValueError):
        return None
    for line in reversed(lines):
        try:
            record = json.loads(line)
        except ValueError:
            continue
        message = record.get("message") if isinstance(record, dict) and record.get("type") == "assistant" else None
        content = message.get("content") if isinstance(message, dict) else None
        if isinstance(content, list):
            text = "\n\n".join(c.get("text", "") for c in content if isinstance(c, dict) and c.get("type") == "text")
            if text.strip():
                return text
    return None


def record(entry):
    """Append one decision to the rolling event log; never fails the hook."""
    try:
        EVENT_LOG.parent.mkdir(exist_ok=True)
        with open(EVENT_LOG, "a+", encoding="utf-8") as log:
            fcntl.flock(log, fcntl.LOCK_EX)  # concurrent hooks share the file
            log.write(json.dumps(entry, ensure_ascii=False) + "\n")
            log.flush()
            if log.tell() > EVENT_LOG_LIMIT:
                log.seek(0)
                lines = log.readlines()
                log.seek(0)
                log.truncate()
                log.writelines(lines[len(lines) // 2:])
        EVENT_LOG.chmod(0o600)
    except Exception:
        pass


AGENTS = ("claude_code", "codex")


def self_reported_agent(argv=None):
    """The installer writes `--agent <name>` into each agent's hook command."""
    argv = sys.argv[1:] if argv is None else argv
    for flag, value in zip(argv, argv[1:]):
        if flag == "--agent" and value in AGENTS:
            return value
    return None


def detect_agent(event, env=None, argv=None):
    """Which agent ran this hook: its self-report first, then an explicit
    override, then the agent's own environment and transcript markers."""
    env = os.environ if env is None else env
    reported = self_reported_agent(argv)
    if reported:
        return reported
    if env.get("NOTIFY_AGENT") in AGENTS:
        return env["NOTIFY_AGENT"]
    ai_agent = env.get("AI_AGENT", "")
    if env.get("CLAUDECODE") == "1" or ai_agent.startswith("claude-code"):
        return "claude_code"
    if env.get("CODEX_THREAD_ID") or ai_agent.startswith("codex"):
        return "codex"
    transcript = str(event.get("transcript_path") or "")
    if "/.claude/" in transcript:
        return "claude_code"
    if "/.codex/" in transcript:
        return "codex"
    return None


def closing_paragraph(message):
    paragraphs = [p.strip() for p in re.split(r"\n\s*\n", plain(message)) if p.strip()]
    return paragraphs[-1] if paragraphs else ""


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
        message = (event.get("last_assistant_message") or last_transcript_text(event.get("transcript_path"))
                   or "The agent finished its turn.")
        # Stop can be a plain-text question when the structured input tool is
        # unavailable. Judge only the closing paragraph, where such a request
        # goes: a finished summary may mention "waiting for your approval" in
        # passing, and that turn is done.
        closing = closing_paragraph(message)
        awaiting = closing.endswith("?") or re.search(
            r"(?i)\b(waiting for your|awaiting your|need your (?:input|approval|confirmation|response))\b", closing)
        endpoint = "/awaiting_user_input" if awaiting else "/all_tasks_finished"
    elif name == "StopFailure":
        # Claude Code: the turn ended on an API error (rate limit, overload, ...).
        details = [event.get(k) for k in ("error", "error_details", "last_assistant_message")]
        message = "\n\n".join(str(d) for d in details if d) or "The turn ended with an error."
        endpoint = "/task_failed"
    elif name in ("UserPromptSubmit", "PostToolUse"):
        session_id = event.get("session_id") or os.environ.get("CODEX_THREAD_ID")
        if not isinstance(session_id, str) or not session_id.strip():
            return None  # /working updates one session's row; there is none to update.
        if name == "PostToolUse":
            # Runs after every tool: only turns a waiting row back to working once
            # the user has answered or approved. It sends nothing else.
            payload = {"session_id": session_id, "only_if_waiting": True}
            if isinstance(event.get("tool_use_id"), str) and event["tool_use_id"]:
                payload["tool_use_id"] = event["tool_use_id"]
            return "/working", payload
        prompt = plain(event.get("prompt") or "") or "Working"
        first_line = next((line.strip() for line in prompt.splitlines() if line.strip()), "Working")
        return "/working", {"session_id": session_id, "title": clip(f"{cwd}: {first_line}", 120),
                            "message": clip(prompt, 4000)}
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
    # Lets only this tool call's completion resume a waiting row (parallel tools).
    if endpoint == "/awaiting_user_input" and isinstance(event.get("tool_use_id"), str) and event["tool_use_id"]:
        payload["tool_use_id"] = event["tool_use_id"]
    return endpoint, payload


def post(event, endpoint, payload):
    agent = detect_agent(event)
    if agent:
        payload["agent"] = agent
    try:
        context = capture_context(event)
        if context:
            payload["context"] = context
    except Exception:
        # Optional context must never suppress an alert.
        pass
    try:
        origin = capture_origin()
        if origin:
            payload["origin"] = origin
    except Exception:
        # A missing terminal hint must never suppress the notification.
        pass
    result = http(endpoint, payload, timeout=5)
    if not (result.get("id") or result.get("updated")):
        raise RuntimeError("Server did not acknowledge the alert")
    return result


def main():
    started = time.monotonic()
    entry = {"time": datetime.now(timezone.utc).isoformat(timespec="milliseconds")}
    try:
        event = json.load(sys.stdin)
        entry.update({k: event[k] for k in ("hook_event_name", "session_id", "tool_name", "tool_use_id")
                      if isinstance(event.get(k), str)})
        alert = notification_for(event)
        entry["action"] = alert[0] if alert else "ignored"
        entry["agent"] = detect_agent(event)
        result = None
        if alert and alert[0] == "/working":
            # Busy updates are informational: never build/start the service (that
            # would block the prompt or tool), and skip them on an older service.
            if (service_version() or 0) >= TASK_STATE_API:
                if alert[1].get("only_if_waiting"):
                    result = http(*alert, timeout=2)  # per-tool: the row already has context/origin
                else:
                    result = post(event, *alert)
            else:
                entry["action"] = "skipped: service lacks task states"
        elif alert:
            if event.get("hook_event_name") == "Stop":
                entry["closing"] = closing_paragraph(alert[1]["message"])[:200]
            ensure_server()
            alert = compatible(*alert)
            entry["action"] = alert[0]
            result = post(event, *alert)
        if isinstance(result, dict):
            row = result.get("notification") if "updated" in result else result
            entry["updated"] = result.get("updated", True)
            if isinstance(row, dict):
                entry.update(state=row.get("state"), id=row.get("id"))
        # A Stop hook must emit JSON, and no hook should change tool permissions.
        print("{}")
    except Exception as error:
        entry["error"] = str(error)
        print(json.dumps({"systemMessage": f"Desktop Notify hook failed: {error}"}))
    finally:
        entry["ms"] = round((time.monotonic() - started) * 1000)
        record(entry)


if __name__ == "__main__":
    main()
