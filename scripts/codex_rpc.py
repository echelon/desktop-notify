"""Small stdio client for the installed Codex app-server's configuration API."""
import json
import queue
import subprocess
import threading


class CodexRPC:
    def __init__(self, codex="codex"):
        self.process = subprocess.Popen([codex, "app-server", "--stdio"], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        self.messages = queue.Queue()
        self.serial = 0
        self.notifications = []

        def reader():
            for line in self.process.stdout:
                try:
                    self.messages.put(json.loads(line))
                except ValueError:
                    pass
            self.messages.put(None)

        threading.Thread(target=reader, daemon=True).start()
        self.call("initialize", {"clientInfo": {"name": "desktop_notify_setup", "version": "1.0"},
                                 "capabilities": {"experimentalApi": True}})
        self.send({"method": "initialized"})

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def receive(self, timeout=30):
        message = self.messages.get(timeout=timeout)
        if message is None:
            raise RuntimeError("Codex app-server exited")
        return message

    def call(self, method, params):
        self.serial += 1
        request_id = self.serial
        self.send({"id": request_id, "method": method, "params": params})
        while True:
            message = self.receive()
            if message.get("id") == request_id and "method" not in message:
                if "error" in message:
                    raise RuntimeError(f"{method}: {message['error']}")
                return message["result"]
            self.notifications.append(message)

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            self.process.wait(timeout=5)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
