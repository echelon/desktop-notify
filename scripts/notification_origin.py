"""Collect best-effort focus hints without opening apps or prompting for access."""
import fcntl
import os
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import time
import uuid

REGISTRATIONS = Path(__file__).resolve().parents[1] / "target/terminal-origins.json"
GHOSTTY = "com.mitchellh.ghostty"
GHOSTTY_SCRIPT = Path(__file__).with_name("ghostty_surface.applescript")
SURFACE_FIELDS = ("window_id", "tab_id", "terminal_id")


def registration_key(origin):
    """Bind a window to its outer TTY and app lifetime, not to every Ghostty window."""
    if origin.get("app_pid") and origin.get("tty"):
        return f'{origin["app_pid"]}:{origin["tty"]}'
    return None


def registered_origin(origin):
    key = registration_key(origin)
    if not key:
        return {}
    try:
        entry = json.loads(REGISTRATIONS.read_text()).get(key, {})
        if entry.get("terminal_app") != origin.get("terminal_app"):
            return {}
        return {field: entry[field] for field in SURFACE_FIELDS
                if isinstance(entry.get(field), str) and entry[field]}
    except (OSError, ValueError, AttributeError):
        return {}


def save_registration(key, entry):
    """Atomically update one window pairing; concurrent hooks share the file."""
    REGISTRATIONS.parent.mkdir(exist_ok=True)
    with REGISTRATIONS.with_suffix(".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            entries = json.loads(REGISTRATIONS.read_text())
        except (OSError, ValueError):
            entries = {}
        entries[key] = entry
        temporary = REGISTRATIONS.with_suffix(f".{os.getpid()}.tmp")
        try:
            temporary.write_text(json.dumps(entries, indent=2) + "\n")
            temporary.chmod(0o600)
            temporary.replace(REGISTRATIONS)
        finally:
            temporary.unlink(missing_ok=True)


def ghostty(*args):
    """Run the fixed read-only lookup; the timeout also kills a stalled
    Automation prompt so a hook can never hang on it."""
    try:
        return subprocess.run(["/usr/bin/osascript", str(GHOSTTY_SCRIPT), *args], capture_output=True,
                              text=True, timeout=3, check=True).stdout.rstrip("\n")
    except (OSError, subprocess.SubprocessError):
        return None


def surface(output):
    ids = (output or "").split("\n")
    if len(ids) == 3 and all(0 < len(i) <= 1024 and i.isprintable() for i in ids):
        return dict(zip(SURFACE_FIELDS, ids))
    return {}


def printable(text, limit):
    return "".join(c for c in text if c.isprintable())[:limit]


def probe_ghostty(tty):
    """Ghostty's scripting API has no TTY or PID, so name this surface: write a
    unique title to its outer TTY, find the terminal carrying it, then restore
    the previous title. Only Ghostty sees the title; tmux is bypassed."""
    titles = ghostty("titles")
    if titles is None:
        return {}
    previous = dict(line.split("\t", 1) for line in titles.split("\n") if "\t" in line)
    marker = "desktop-notify-" + uuid.uuid4().hex
    try:
        fd = os.open(tty, os.O_WRONLY | os.O_NOCTTY | os.O_NONBLOCK)
    except OSError:
        return {}
    found = {}
    try:
        # One small write per sequence so it cannot split around other output.
        os.write(fd, f"\033]2;{marker}\007".encode())
        for _ in range(8):
            time.sleep(0.05)  # Ghostty applies title changes asynchronously.
            found = surface(ghostty("title", marker))
            if found:
                break
        # Titles are terminal data: restore them as text, never as sequences.
        restore = printable(previous.get(found.get("terminal_id"), ""), 1024)
        os.write(fd, f"\033]2;{restore}\007".encode())
    except OSError:
        pass
    finally:
        os.close(fd)
    return found


def ghostty_origin(origin):
    """Exact Ghostty window/tab/terminal for this origin's outer TTY, cached per
    Ghostty process and TTY. A cached terminal that still exists is still this
    surface: its PTY cannot be reused while the surface holds it open."""
    key = registration_key(origin)
    if not key or origin.get("terminal_app") != GHOSTTY:
        return {}
    cached = registered_origin(origin)
    if cached.get("terminal_id"):
        current = surface(ghostty("terminal", cached["terminal_id"]))
        if current:
            return current
    found = probe_ghostty(origin["tty"])
    if found:
        try:
            save_registration(key, {"terminal_app": GHOSTTY, **found})
        except OSError:
            pass  # The pairing still applies to this alert.
        return found
    # A manual register_terminal.py window pairing remains a fallback.
    return cached

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
    display = lambda form: command(base + ["display-message", "-p", "-t", pane, form])
    ids = display("#{pid} #{session_id} #{window_id} #{window_index}").split(" ")
    session = None
    if len(ids) == 4 and ids[0].isdigit() and ids[1][:1] == "$" and ids[2][:1] == "@" and ids[3].isdigit():
        session = ids[1]
        origin.update(tmux_server_pid=int(ids[0]), tmux_session=ids[1], tmux_window=ids[2],
                      tmux_window_index=int(ids[3]))
        # Names are free text, so each is read on its own rather than split.
        for field, form in (("tmux_session_name", "#{session_name}"), ("tmux_window_name", "#{window_name}")):
            name = printable(display(form), 200).strip()
            if name:
                origin[field] = name
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


def capture_origin(env=None, include_registered=True):
    if sys.platform != "darwin":
        return None
    env = os.environ if env is None else env
    processes = process_table()
    parent = os.getppid()
    chain = list(ancestry(processes, parent))
    source_pid = next((pid for pid, _, exe in chain if Path(exe).name in ("codex", "claude")), parent)
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
    if include_registered:
        if env.get("NOTIFY_GHOSTTY_PROBE") == "0":
            origin.update(registered_origin(origin))
        else:
            origin.update(ghostty_origin(origin) or registered_origin(origin))
    # Explicit per-session overrides for callers with a known scripting ID.
    # Ghostty 1.3's AppleScript UUID is not its newer core surface ID.
    for field in ("terminal_app", "terminal_id", "window_id", "window_title", "tty"):
        if env.get("NOTIFY_" + field.upper()):
            origin[field] = env["NOTIFY_" + field.upper()]
    return origin
