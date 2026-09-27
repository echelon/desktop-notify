# Hooks, discovery, and development helpers

Inherit the root rules. Python uses four-space indentation and the standard
library; `test_ui.mjs` uses two-space JavaScript and Node's built-in test runner.
Resolve repository paths from `__file__`, since hooks run from other directories.

## Hook contract and startup

- `codex_hook.py` consumes stdin JSON and emits valid hook JSON on stdout. Success
  is `{}`; failures use `systemMessage`. Hooks must never grant/deny permissions,
  block a completed turn through a decision, or mix diagnostic text into JSON.
- Preserve Stop, PermissionRequest, and structured-question PreToolUse handling,
  including `request_user_input_async` and namespaced tool names. A Stop means one
  turn completed. Plain-text question detection is only a fallback heuristic.
- The async question tool returns before the user answers; ignore that tool's
  PostToolUse instead of resuming its waiting row. UserPromptSubmit handles the
  next user message. Blocking question tools still resume on PostToolUse.
- Forward the event's `session_id`, falling back to `CODEX_THREAD_ID`; never use
  cwd as session identity. Preserve actual questions/outcomes and API text limits.
- Probe `/health` for identity and a supported API version. Preserve the file
  lock and second health check around cold build/start so simultaneous hooks
  start one service. Reuse healthy services and reopen a missing desktop app.
- Keep local HTTP independent of proxy environment variables. Use timeouts,
  detached service startup, and `target/desktop-notify.log` for build/start output.
  Keep `build.py` as the common build/bundle entry point.
- Pass subprocess arguments as lists. Never run conversation data as shell code.

## Best-effort metadata

- Shared Codex daemons inherit another client's terminal environment. Never use
  that environment as a session origin. `codex_terminal.py` transparently relays
  local TUI WebSocket traffic to the same daemon and registers exact session IDs
  before forwarding turn requests/start-resume-fork responses. `session_origin.py`
  stores only allowed terminal hints plus client PID/start time; hooks validate
  that live client before discovery. Never infer a binding from cwd, timing,
  conversation text, or the frontmost window. An unbound detached daemon sends
  `origin: {}` to replace stale hints, while notifications still work.
- The terminal bridge must preserve frames and keep conversation/approval data
  out of logs. Metadata failures must not interrupt Codex. Pass utility commands
  and explicit remote hosts through; keep the shared daemon and top-level
  computer-use `notify` configuration. Its shell function is installed by
  `install_terminal_bridge.py`, with preview and backups.
- Keep context/origin collection independent: failure of either must not suppress
  the alert. Discovery must not execute repository code or call models/network
  services. Origin collection must not open apps; the Ghostty probe is the only
  scripting it performs, and a stalled Automation prompt must be killed by its timeout.
- Read repo metadata from manifests and README fallbacks; handle worktree `.git`
  files. Bound reads (currently 64 KiB metadata files, last 8 MiB of transcripts).
- Transcripts are an unstable input format. Tolerate missing files, malformed
  records, and unsupported shapes. Use existing request/progress/plan/goal text
  instead of inventing summaries. Keep explicit context and `NOTIFY_*` overrides.
- For tmux, find the GUI through the attached client for the originating session,
  not merely the tmux server's ancestry; keep socket, stable pane, and client TTY.
- Ghostty surfaces are identified by `probe_ghostty`: write a random OSC 2 title
  to the outer TTY (tmux client TTY), find it via the fixed read-only
  `ghostty_surface.applescript` (arguments via argv), then restore the previous
  title as printable text. Only probe when Ghostty hosts the session, bound each
  lookup (3 s), cache per app PID and TTY, and revalidate the cached terminal
  before reuse. Lock the complete probe per app/TTY without blocking another
  hook; a scripting failure ends retries. Honor `NOTIFY_GHOSTTY_PROBE=0`. Tests
  must patch `ghostty` and TTY writes; never script the real Ghostty or write to a
  real TTY in tests.
- Inside tmux, report the server PID, session/window IDs, index, and names; read
  free-text names separately rather than splitting them from one format string.
- Manual terminal registration is explicit, foreground Ghostty pairing keyed by app PID
  and outer TTY. Store it under ignored `target/terminal-origins.json`, using a lock
  and atomic replacement. Do not assign one registration to every Ghostty window
  or mistake a core surface ID for a scripting terminal ID.

## Global configuration and verification

- `codex_hook.py` also serves Claude Code, whose Stop/PermissionRequest/
  PreToolUse payloads share these field names. When Stop lacks
  `last_assistant_message`, use the newest assistant text in the Claude JSONL
  transcript. `notification_context.py` reads both transcript formats; Claude
  `isMeta`, tool-result, and slash-command records are not user requests.
- `install_hooks.py --claude` writes the same hooks to Claude Code's
  `settings.json` (honor `CLAUDE_CONFIG_DIR`); it has no trust step. It must
  remove legacy `agent_notify*.sh` hooks, which clear every row via `/stop`.
- `install_hooks.py` previews by default. Preserve unrelated hooks/configuration,
  including the separate top-level notify integration, and create timestamped
  backups before installation. Use `codex_rpc.py` and the installed Codex config
  API to trust the exact returned definition hashes, then verify enabled/trusted
  status. Do not bypass trust or hardcode hashes.
- Editing repository scripts does not itself require installing hooks: installed
  commands load the script on each invocation. Definitions changed by installation
  require a new Codex session. Honor `CODEX_HOME` in installer paths.
- `install_hooks.py --with-guidance` additionally manages a marked section of
  Codex's global `AGENTS.md` from `docs/codex/AGENTS.md`, expanding this checkout's
  path. Preserve personal instructions, back up existing content, keep the sibling
  relative CLAUDE symlink, and reject malformed markers or conflicting links
  before changing configuration. Preview and verification support the same flag.
- `codex_hook.py` appends every decision to the rolling `target/hook-events.jsonl`
  (never failing the hook) and rotates the service log on start. Tests must patch
  `EVENT_LOG` to a temporary file.
- Run `python3 -m unittest discover -s scripts -p 'test_*.py'` for Python changes;
  run `node --test scripts/test_ui.mjs` for the UI test harness. Mock local HTTP,
  processes, transcripts, environment, and home/config files in unit tests.
- Real integration helpers play audio and depend on installed hooks;
  `test_codex_live.py` also uses the configured account/model, and smoke-test
  `--restart` discards service state. Do not treat them as ordinary unit tests.
- Test alerts need unique session IDs and cleanup in `finally`. Dismiss the test's
  current IDs so cleanup preserves other sessions and cannot clear replacements.
