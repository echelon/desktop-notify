#!/usr/bin/env python3
"""Optional live Codex integration test (uses the user's configured model).

It asks one synthetic question, verifies the actual PreToolUse hook, answers it,
then verifies the Stop hook. No model-directed file or shell access is needed.
"""
import time

import codex_hook as hook
from codex_rpc import CodexRPC
from install_hooks import ROOT
from smoke_test import eventually, session_notification, clear_sessions


def run():
    hook.ensure_server()
    thread_id = None
    answered = False
    completed = False
    try:
        with CodexRPC() as rpc:
            started = rpc.call("thread/start", {
                "cwd": str(ROOT), "ephemeral": True, "sandbox": "read-only", "approvalPolicy": "never",
                "config": {"notify": []},
                "developerInstructions": "This is a notification hook test. Use only request_user_input. Do not inspect or modify files or run shell commands."
            })
            thread_id = started["thread"]["id"]
            rpc.call("turn/start", {
                "threadId": thread_id,
                "collaborationMode": {"mode": "plan", "settings": {"model": started["model"]}},
                "input": [{"type": "text", "text": "Test the input notification hook: call request_user_input once with question 'Continue the Desktop Notify integration test?' and two choices, Continue and Cancel. After receiving Continue, finish with exactly 'Desktop Notify live question and completion hooks passed.'"}]
            })
            deadline = time.monotonic() + 180
            while time.monotonic() < deadline:
                message = rpc.notifications.pop(0) if rpc.notifications else rpc.receive(timeout=60)
                method = message.get("method", "")
                if "hook/" in method:
                    print(method, message.get("params"), flush=True)
                if method == "item/tool/requestUserInput":
                    params = message["params"]
                    notification = eventually(lambda: (n if (n := session_notification(thread_id)) and
                                                       n["kind"] == "awaiting_user_input" else None))
                    assert "Continue the Desktop Notify integration test" in notification["message"]
                    eventually(lambda: hook.http("/state")["audio"]["loop_name"] == "await")
                    print("PASS actual Codex request_user_input -> awaiting notification and sound", flush=True)
                    answers = {q["id"]: {"answers": ["Continue"]} for q in params["questions"]}
                    rpc.send({"id": message["id"], "result": {"answers": answers}})
                    answered = True
                elif method == "turn/completed":
                    assert message["params"]["turn"]["status"] == "completed", message
                    completed = True
                    break
                elif "id" in message and "method" in message:
                    raise AssertionError(f"Unexpected server request: {method}")
            assert answered and completed, "Codex did not complete the question test"
            eventually(lambda: (n := session_notification(thread_id)) and n["kind"] == "all_tasks_finished")
            # Another session may still have a question with sound priority.
            assert not session_notification(thread_id)["silenced"]
            print("PASS actual Codex Stop -> completion notification and sound", flush=True)
    finally:
        if thread_id:
            clear_sessions([thread_id])


if __name__ == "__main__":
    run()
