"""Session-to-terminal bindings for Codex clients of the shared app-server.

The terminal-side bridge writes these before forwarding a turn to the daemon.
Hooks read by session ID and validate the client process lifetime. No cwd,
foreground window, daemon environment, or conversation text is used as identity.
"""
import hashlib
import json
from pathlib import Path
import subprocess

DIRECTORY = Path(__file__).resolve().parents[1] / "target/session-origins"
ENV_KEYS = ("TMUX", "TMUX_PANE", "TERM_PROGRAM", "ITERM_SESSION_ID",
            "NOTIFY_GHOSTTY_PROBE", "NOTIFY_TERMINAL_APP", "NOTIFY_TERMINAL_ID",
            "NOTIFY_WINDOW_ID", "NOTIFY_WINDOW_TITLE", "NOTIFY_TTY")


def process_start(pid):
    try:
        return subprocess.run(["/bin/ps", "-p", str(pid), "-o", "lstart="],
                              capture_output=True, text=True, check=True, timeout=2).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def binding_path(session_id):
    if not isinstance(session_id, str) or not 0 < len(session_id.encode()) <= 256 or not session_id.isprintable():
        raise ValueError("Invalid session ID")
    return DIRECTORY / (hashlib.sha256(session_id.encode()).hexdigest() + ".json")


def save(session_id, pid, started, env):
    if not started or not isinstance(pid, int) or pid <= 1:
        raise ValueError("A live terminal client is required")
    path = binding_path(session_id)
    DIRECTORY.mkdir(parents=True, exist_ok=True)
    data = {"version": 1, "session_id": session_id, "pid": pid, "started": started,
            "env": {key: env[key] for key in ENV_KEYS if isinstance(env.get(key), str)}}
    # Each terminal bridge has one reader per direction; unique temporary names
    # also let separate clients of the same session update atomically.
    import tempfile
    with tempfile.NamedTemporaryFile(mode="w", dir=DIRECTORY, delete=False) as f:
        temporary = Path(f.name)
        try:
            json.dump(data, f)
            f.flush()
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)


def load(session_id, processes):
    try:
        path = binding_path(session_id)
        with path.open() as f:
            data = json.loads(f.read(16385))
        pid = data.get("pid")
        process = processes.get(pid)
        if (data.get("version") != 1 or data.get("session_id") != session_id
                or not isinstance(pid, int) or not process
                or Path(process[2]).name != "codex" or process[1] in ("??", "?", "-")
                or not data.get("started") or process_start(pid) != data["started"]
                or not isinstance(data.get("env"), dict)):
            return None
        return pid, {k: v for k, v in data["env"].items() if k in ENV_KEYS and isinstance(v, str)}
    except (OSError, ValueError, TypeError, AttributeError):
        return None
