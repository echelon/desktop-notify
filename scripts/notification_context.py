"""Best-effort local session context. Never execute repository code or call a model."""
import json
import os
from pathlib import Path
import re
import tomllib

LIMITS = {"cwd": 4096, "work_arc": 2000, "current_ask": 2000,
          "repo_name": 200, "repo_description": 1000}
MAX_FILE_BYTES = 64 * 1024
MAX_TRANSCRIPT_BYTES = 8 * 1024 * 1024


def text(value, limit):
    if not isinstance(value, str):
        return None
    # Keep paths and prose literal; flatten whitespace for compact display.
    value = " ".join(value.split())
    value = "".join(c for c in value if ord(c) >= 32 and ord(c) != 127)
    return value if len(value) <= limit else value[:limit - 1].rstrip() + "…"


def directory_text(value):
    if not isinstance(value, str) or len(value) > LIMITS["cwd"] or any(ord(c) < 32 or ord(c) == 127 for c in value):
        return None
    return value.strip()


def read_small(path):
    try:
        if path.is_file() and path.stat().st_size <= MAX_FILE_BYTES:
            return path.read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        pass
    return ""


def readme_description(root):
    source = next((content for name in ("README.md", "README", "readme.md", "README.rst")
                   if (content := read_small(root / name))), "")
    source = re.sub(r"\A---\s*\n.*?\n---\s*\n", "", source, count=1, flags=re.S)
    paragraph = []
    for line in source.splitlines():
        line = line.strip()
        if not line:
            if paragraph:
                break
            continue
        if line.startswith(("#", "[!", "![", "<", "===", "---")):
            if paragraph:
                break
            continue
        if line.startswith(("```", "~~~", "- ", "* ", "|")):
            break
        paragraph.append(line)
    value = " ".join(paragraph)
    value = re.sub(r"\[([^]]+)\]\([^)]+\)", r"\1", value).replace("**", "").replace("`", "")
    return text(value, LIMITS["repo_description"])


def repository_context(cwd):
    if not cwd or not Path(cwd).is_absolute():
        return {}
    directory = Path(cwd)
    if not directory.is_dir():
        return {}
    # .git may be a directory or a worktree pointer file. No git/network calls.
    root = next((p for p in (directory, *directory.parents) if (p / ".git").exists()), None)
    if root is None:
        return {}
    result = {"repo_name": text(root.name, LIMITS["repo_name"])}
    for name, section in (("package.json", None), ("Cargo.toml", "package"), ("pyproject.toml", "project")):
        try:
            source = read_small(root / name)
            if not source:
                continue
            manifest = json.loads(source) if name.endswith(".json") else tomllib.loads(source)
            package = manifest if section is None else manifest.get(section, {})
            for field in ("name", "description"):
                value = text(package.get(field), LIMITS["repo_" + field])
                if value:
                    result["repo_" + field] = value
            if "repo_description" in result:
                break
        except (ValueError, AttributeError, TypeError):
            continue
    if "repo_description" not in result:
        description = readme_description(root)
        if description:
            result["repo_description"] = description
    return result


def message_text(item):
    return "\n".join(c.get("text", "") for c in item.get("content", [])
                     if isinstance(c, dict) and c.get("type") in ("input_text", "output_text", "text")
                     and isinstance(c.get("text"), str))


def user_request(value):
    if not isinstance(value, str):
        return None
    # Bootstrap context is sent with the user role too; it isn't the user's task.
    # Claude Code also records slash-command echoes/output as user text.
    if value.lstrip().startswith(("# AGENTS.md instructions", "<INSTRUCTIONS>", "<command-", "<local-command-")):
        return None
    value = re.sub(r"<(environment_context|environment_details|user_instructions|system-reminder)>.*?</\1>", "", value, flags=re.S)
    return text(value, LIMITS["current_ask"])


def transcript_context(path):
    """Tolerate changing transcript formats; a missing/unreadable tail is optional."""
    if not isinstance(path, str) or not path:
        return {}
    try:
        file = Path(path)
        if not file.is_file():
            return {}
        with file.open("rb") as stream:
            size = os.fstat(stream.fileno()).st_size
            stream.seek(max(0, size - MAX_TRANSCRIPT_BYTES))
            data = stream.read(MAX_TRANSCRIPT_BYTES)
        if size > MAX_TRANSCRIPT_BYTES:
            data = data.partition(b"\n")[2]  # discard the partial first record
    except OSError:
        return {}
    result = {}
    for line in data.splitlines():
        try:
            record = json.loads(line)
            if record.get("type") == "user" and not record.get("isMeta"):
                # Claude Code: prompts are string content; tool results are lists
                # containing tool_result blocks and are not user requests.
                content = record.get("message", {}).get("content")
                if isinstance(content, list) and all(isinstance(c, dict) and c.get("type") == "text" for c in content):
                    content = "\n".join(c.get("text", "") for c in content)
                request = user_request(content)
                if request:
                    result = {"current_ask": request}
                continue
            item = record.get("payload", {})
            if not isinstance(item, dict):
                continue
            kind = item.get("type")
            request = None
            if record.get("type") == "response_item" and kind == "message" and item.get("role") == "user":
                request = user_request(message_text(item))
            elif record.get("type") == "event_msg" and kind == "user_message":
                request = user_request(item.get("message"))
            if request:
                result = {"current_ask": request}
            elif record.get("type") == "response_item" and "current_ask" in result:
                if kind == "message" and item.get("role") == "assistant" and (item.get("phase") or item.get("channel")) == "commentary":
                    # The initial agent summary is its interpretation of this run.
                    arc = text(message_text(item), LIMITS["work_arc"])
                    if arc and "work_arc" not in result:
                        result["work_arc"] = arc
                elif kind == "function_call":
                    name = item.get("name", "").split(".")[-1]
                    args = json.loads(item.get("arguments", "{}"))
                    if name == "create_goal":
                        arc = text(args.get("objective"), LIMITS["work_arc"])
                        if arc:
                            result["work_arc"] = arc
                    elif name == "update_plan" and "work_arc" not in result:
                        arc = text(args.get("explanation"), LIMITS["work_arc"])
                        if arc:
                            result["work_arc"] = arc
        except (ValueError, AttributeError, TypeError):
            continue
    return result


def capture_context(event, env=None):
    env = os.environ if env is None else env
    # Explicit hook context/env values take precedence over best-effort discovery.
    explicit = event.get("context") if isinstance(event.get("context"), dict) else {}
    cwd = directory_text(explicit.get("cwd", env.get("NOTIFY_CWD", event.get("cwd"))))
    result = transcript_context(event.get("transcript_path"))
    prompt = user_request(event.get("prompt"))
    if prompt:
        result["current_ask"] = prompt
    if cwd:
        result["cwd"] = cwd
        result.update(repository_context(cwd))
    for field, limit in LIMITS.items():
        raw = explicit.get(field, env.get("NOTIFY_" + field.upper()))
        value = directory_text(raw) if field == "cwd" else text(raw, limit)
        if value is not None:
            result[field] = value
    return result
