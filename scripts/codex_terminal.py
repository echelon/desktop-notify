#!/usr/bin/env python3
"""Run the normal Codex TUI through a terminal-aware local daemon connection.

The shared daemon, models, tools and hooks are unchanged. This Unix-socket
WebSocket relay observes session IDs only, and forwards messages byte-for-byte.
It binds each session to this terminal before the daemon can run its hooks.
"""
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import threading

import session_origin

MAX_OBSERVE = 16 * 1024 * 1024


class Bindings:
    def __init__(self, pid, started, env):
        self.pid, self.started, self.env = pid, started, env
        self.pending = set()
        self.lock = threading.Lock()

    def observe(self, message, from_client):
        if not isinstance(message, dict):
            return
        with self.lock:
            method, request_id = message.get("method"), message.get("id")
            if not isinstance(request_id, (str, int)):
                return
            session = None
            params = message.get("params") or {}
            if from_client and isinstance(params, dict):
                if method in ("thread/start", "thread/resume", "thread/fork"):
                    self.pending.add(request_id)
                elif method in ("turn/start", "turn/steer", "review/start"):
                    session = params.get("threadId")
            elif not from_client and method is None and request_id in self.pending:
                self.pending.remove(request_id)
                result = message.get("result") or {}
                thread = result.get("thread") if isinstance(result, dict) else None
                if isinstance(thread, dict):
                    session = thread.get("id")
            if session:
                try:
                    session_origin.save(session, self.pid, self.started, self.env)
                except (OSError, ValueError):
                    # Optional notification metadata must never block Codex.
                    # An unregistered daemon hook reports an empty origin.
                    pass


def exact(stream, length):
    out = bytearray()
    while len(out) < length:
        chunk = stream.read(length - len(out))
        if not chunk:
            raise EOFError()
        out.extend(chunk)
    return bytes(out)


def headers(stream):
    data = bytearray()
    while not data.endswith(b"\r\n\r\n"):
        data.extend(exact(stream, 1))
        if len(data) > 65536:
            raise ValueError("Oversized WebSocket handshake")
    return bytes(data)


def relay(stream, destination, bindings, from_client):
    """Observe text messages, including fragmentation, without rewriting frames.

    Large/binary/extension frames still stream through with bounded memory.
    Control frames may interleave fragmented text. Never log payloads: these
    connections also carry conversation text and approval responses.
    """
    message = None
    while True:
        header = exact(stream, 2)
        opcode, final = header[0] & 15, bool(header[0] & 128)
        length = header[1] & 127
        if length in (126, 127):
            size = 2 if length == 126 else 8
            extra = exact(stream, size)
            length = int.from_bytes(extra, "big")
            header += extra
        mask = exact(stream, 4) if header[1] & 128 else b""
        if opcode == 1:
            message = bytearray() if not header[0] & 112 else None
        elif opcode not in (0, 8, 9, 10):
            message = None
        observe = opcode in (0, 1) and message is not None and len(message) + length <= MAX_OBSERVE
        if opcode in (0, 1) and not observe:
            message = None
        # Hold ordinary frames until registration completes; otherwise the
        # daemon can start a hook before its origin exists. Huge frames stream.
        if observe:
            payload = exact(stream, length)
            message.extend(bytes(c ^ mask[i % 4] for i, c in enumerate(payload)) if mask else payload)
            if final:
                try:
                    parsed = json.loads(message)
                except (ValueError, UnicodeError):
                    parsed = None
                if parsed is not None:
                    bindings.observe(parsed, from_client)
                message = None
            destination.sendall(header + mask + payload)
        else:
            destination.sendall(header + mask)
            while length:
                chunk = exact(stream, min(length, 65536))
                destination.sendall(chunk)
                length -= len(chunk)
        if opcode == 8:
            return


def bridge(client, upstream_path, bindings):
    upstream = socket.socket(socket.AF_UNIX)
    sockets = (client, upstream)
    try:
        upstream.settimeout(10)
        upstream.connect(str(upstream_path))
        client.settimeout(10)
        incoming, outgoing = client.makefile("rb"), upstream.makefile("rb")
        upstream.sendall(headers(incoming))
        response = headers(outgoing)
        client.sendall(response)
        if not response.startswith(b"HTTP/1.1 101 "):
            return
        for connection in sockets:
            connection.settimeout(None)

        def forward(stream, destination, from_client):
            try:
                relay(stream, destination, bindings, from_client)
            except (EOFError, OSError, ValueError):
                pass
            finally:
                for connection in sockets:
                    try:
                        connection.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass

        worker = threading.Thread(target=forward, args=(outgoing, client, False), daemon=True)
        worker.start()
        forward(incoming, upstream, True)
        worker.join(timeout=2)
        incoming.close()
        outgoing.close()
    finally:
        for connection in sockets:
            connection.close()


def interactive(args):
    """Only route local TUI invocations; utility commands and remote hosts pass through."""
    values = {"-c", "--config", "--enable", "--disable", "-C", "--cd", "-m", "--model",
              "-p", "--profile", "-s", "--sandbox", "-a", "--ask-for-approval", "-i", "--image", "--add-dir"}
    commands = {"exec", "e", "review", "login", "logout", "mcp", "plugin", "app-server",
                "remote-control", "app", "completion", "update", "doctor", "sandbox", "debug",
                "apply", "a", "queue", "archive", "delete", "migrate-rollouts", "unarchive",
                "cloud", "exec-server", "features", "help"}
    skip, first = False, None
    for arg in args:
        if skip:
            skip = False
        elif arg in ("--remote", "--no-daemon", "--help", "-h", "--version", "-V", "--worktree", "--oss", "--profile", "-p") or arg.startswith(("--remote=", "--profile=")):
            return False
        elif arg in values:
            skip = True
        elif not arg.startswith("-") and first is None:
            first = arg
    return first not in commands


def without_old_bridge(args):
    """Codex prints a reconnect command containing our temporary socket.

    A new terminal must own a fresh relay/binding, even if the old relay is
    still alive. Only rewrite our private local paths; real remotes pass through.
    """
    def ours(value):
        return value.startswith("unix:///tmp/dn-") and value.endswith("/codex.sock")
    result, i = [], 0
    while i < len(args):
        if args[i] == "--remote" and i + 1 < len(args) and ours(args[i + 1]):
            i += 2
        elif args[i].startswith("--remote=") and ours(args[i].split("=", 1)[1]):
            i += 1
        else:
            result.append(args[i])
            i += 1
    return result


def main():
    args = without_old_bridge(sys.argv[1:])
    codex = shutil.which("codex")
    if not codex:
        raise SystemExit("codex is not on PATH")
    if not sys.stdin.isatty() or not interactive(args):
        os.execv(codex, [codex, *args])
    # Explicit remote mode otherwise defaults new tasks to the daemon's cwd.
    if not any(a in ("-C", "--cd") or a.startswith(("--cd=", "-C")) for a in args):
        args = ["--cd", os.getcwd(), *args]
    home = Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))
    upstream = home / "app-server-control/app-server-control.sock"
    if not upstream.exists():
        subprocess.run([codex, "app-server", "daemon", "start"], check=True)
    # macOS Unix socket paths are short (104 bytes). Private temporary directory
    # permissions restrict this relay to the current user; no TCP listener.
    with tempfile.TemporaryDirectory(prefix="dn-", dir="/tmp") as directory:
        listener = socket.socket(socket.AF_UNIX)
        path = str(Path(directory) / "codex.sock")
        listener.bind(path)
        listener.listen()
        listener.settimeout(0.5)
        process = subprocess.Popen([codex, "--remote", "unix://" + path, *args])
        bindings = Bindings(process.pid, session_origin.process_start(process.pid), dict(os.environ))

        def accept():
            while process.poll() is None:
                try:
                    client, _ = listener.accept()
                except socket.timeout:
                    continue
                except OSError:
                    return
                threading.Thread(target=bridge, args=(client, upstream, bindings), daemon=True).start()

        threading.Thread(target=accept, daemon=True).start()
        try:
            # Both processes share the terminal process group. Codex handles
            # Ctrl-C itself; keep the relay alive while it decides whether to exit.
            while True:
                try:
                    return process.wait()
                except KeyboardInterrupt:
                    continue
        finally:
            listener.close()


if __name__ == "__main__":
    sys.exit(main())
