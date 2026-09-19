"""Collect best-effort focus hints without opening apps or prompting for access."""
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys

TERMINALS = {
    "ghostty": "com.mitchellh.ghostty",
    "Apple_Terminal": "com.apple.Terminal",
    "iTerm.app": "com.googlecode.iterm2",
    "iTerm2": "com.googlecode.iterm2",
    "WezTerm": "com.github.wez.wezterm",
}


def command(args):
    try:
        return subprocess.run(args, capture_output=True, text=True, timeout=2, check=True).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def process_table():
    processes = {}
    for line in command(["/bin/ps", "-axo", "pid=,ppid=,tty=,comm="]).splitlines():
        fields = line.strip().split(None, 3)
        if len(fields) == 4 and fields[0].isdigit() and fields[1].isdigit():
            processes[int(fields[0])] = (int(fields[1]), fields[2], fields[3])
    return processes


def ancestry(processes, pid):
    seen = set()
    while pid in processes and pid > 1 and pid not in seen:
        seen.add(pid)
        parent, tty, executable = processes[pid]
        yield pid, tty, executable
        pid = parent


def application(processes, pid):
    for app_pid, _, executable in ancestry(processes, pid):
        if ".app/Contents/MacOS/" in executable:
            bundle = Path(executable.split(".app/", 1)[0] + ".app")
            try:
                with (bundle / "Contents/Info.plist").open("rb") as f:
                    identifier = plistlib.load(f).get("CFBundleIdentifier")
                if identifier:
                    return {"terminal_app": identifier, "app_pid": app_pid}
            except (OSError, ValueError, plistlib.InvalidFileException):
                pass
    return {}


def tmux_context(env):
    value, pane = env.get("TMUX", ""), env.get("TMUX_PANE", "")
    if not value or not pane.startswith("%") or not pane[1:].isdigit():
        return {}, None
    socket = value.rsplit(",", 2)[0]
    if not socket.startswith("/"):
        return {}, None
    origin = {"tmux_socket": socket, "tmux_pane": pane}
    tmux = shutil.which("tmux") or next((p for p in ("/opt/homebrew/bin/tmux", "/usr/local/bin/tmux") if Path(p).is_file()), "tmux")
    base = [tmux, "-S", socket]
    session = command(base + ["display-message", "-p", "-t", pane, "#{session_id}"])
    clients = []
    for line in command(base + ["list-clients", "-F", "#{client_pid}|#{client_tty}|#{session_id}|#{client_activity}"]).splitlines():
        fields = line.split("|")
        if len(fields) == 4 and fields[0].isdigit() and fields[2] == session and fields[3].isdigit():
            clients.append((int(fields[3]), int(fields[0]), fields[1]))
    if clients:
        _, pid, tty = max(clients)
        origin.update(tty=tty, tmux_client=tty)
        return origin, pid
    return origin, None


def capture_origin(env=None):
    if sys.platform != "darwin":
        return None
    env = os.environ if env is None else env
    processes = process_table()
    parent = os.getppid()
    chain = list(ancestry(processes, parent))
    source_pid = next((pid for pid, _, exe in chain if Path(exe).name == "codex"), parent)
    origin = {"pid": source_pid}
    tmux, client_pid = tmux_context(env)
    origin.update(tmux)
    origin.update(application(processes, client_pid or source_pid))
    if not tmux:
        tty = next((tty for _, tty, _ in chain if tty not in ("??", "?", "-")), None)
        if tty:
            origin["tty"] = tty if tty.startswith("/dev/") else "/dev/" + tty
        terminal = TERMINALS.get(env.get("TERM_PROGRAM"))
        if terminal:
            origin.setdefault("terminal_app", terminal)
        # iTerm's environment includes a positional prefix; AppleScript uses
        # the stable ID following the colon. It may be stale inside tmux.
        if env.get("ITERM_SESSION_ID"):
            origin["terminal_id"] = env["ITERM_SESSION_ID"].split(":", 1)[-1]
    # Explicit per-session overrides for callers with a known scripting ID.
    # Ghostty 1.3's AppleScript UUID is not its newer core surface ID.
    for field in ("terminal_app", "terminal_id", "window_id", "window_title", "tty"):
        if env.get("NOTIFY_" + field.upper()):
            origin[field] = env["NOTIFY_" + field.upper()]
    return origin
