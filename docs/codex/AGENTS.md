# Desktop Notify integration for Codex

Desktop Notify is installed from `@DESKTOP_NOTIFY_ROOT@`. Its lifecycle hooks
report status automatically; use the normal question/approval tools and finish
turns normally. Do not duplicate their status posts with curl or call `/stop`
from agent work. The separate top-level Codex `notify` command belongs to the
computer-use integration and must be preserved.

## Using the bindings

- `UserPromptSubmit` marks this session `working`. Structured questions
  (`request_user_input`, `request_user_input_async`) and permission requests mark
  it `input_needed`; include the actual question or useful approval justification.
- Matching `PostToolUse` resumes a waiting row. The async question tool's immediate
  return is ignored because it is not an answer; the next user prompt resumes it.
- `Stop` reports `done` unless the closing paragraph asks a question or requests
  input. Put a real outstanding request in that paragraph; end completed work
  with a declarative outcome. Do not invent an extra question to trigger an alert.
- Each independent Codex session uses its event `session_id`, never cwd, tmux
  window, or PID. `tool_use_id` keeps unrelated parallel tools from cancelling
  questions. Codex subagents share the parent's hook session ID; do not install
  SubagentStop completion hooks that overwrite the parent's row.
- Hooks capture repo/task context from the event and transcript. Write a useful
  initial progress update and final outcome. No manual metadata posts are needed.

## Exact terminal focus

For shared-daemon Codex, the terminal-side `scripts/codex_terminal.py` bridge
binds the exact session ID to its live terminal client before hooks run. Hooks
validate the client PID/start time and use its terminal hints from ignored
`target/session-origins/`. Never use the shared daemon's inherited `TMUX_PANE`
as a session origin or guess by cwd. Keep the shared daemon enabled. Install the
shell function with `python3 @DESKTOP_NOTIFY_ROOT@/scripts/install_terminal_bridge.py --install`;
new shells load it through the shared aliases loader, and existing sessions
register when resumed through it. Claude keeps using direct process ancestry.

Hooks discover the originating tmux socket/pane/server and its most recently
active attached client. That client's outer TTY and process ancestry identify
Ghostty. A short, locked OSC-title probe maps the TTY to Ghostty's AppleScript
window/tab/terminal IDs; later hooks revalidate the cached terminal. Do not guess
from the frontmost window, persist a hardcoded pane/TTY in global config, or use
Ghostty's core surface ID as its scripting terminal ID.

Focus selects the recorded pane on the recorded tmux server, then raises the
recorded terminal. It does not dismiss or mute. For diagnosis, read
`@DESKTOP_NOTIFY_ROOT@/AGENTS.md` and inspect the row's `origin` at
`http://127.0.0.1:43110/notifications`. Run capture or manual registration from the
intended terminal; a detached app/remote shell cannot infer an unrelated local
Ghostty surface. For manual pairing, run
`python3 @DESKTOP_NOTIFY_ROOT@/scripts/register_terminal.py --terminal --test`
there. `NOTIFY_GHOSTTY_PROBE=0` disables automatic probing.

## Making, installing, and verifying changes

From `@DESKTOP_NOTIFY_ROOT@`:

```sh
python3 scripts/install_hooks.py --with-guidance            # Preview
python3 scripts/install_hooks.py --install --with-guidance  # Back up, install, trust, verify
python3 scripts/install_hooks.py --verify --with-guidance   # Verify hooks and this guidance
python3 -m unittest discover -s scripts -p 'test_*.py'
```

The installer owns the five definitions in `$CODEX_HOME/hooks.json` (default
`~/.codex/hooks.json`) and trusts their exact hashes through the installed Codex
config API. It preserves unrelated settings and adds this managed section to
`$CODEX_HOME/AGENTS.md`; edit its source in `docs/codex/AGENTS.md` and reinstall.
Start new Codex sessions after definition or guidance changes. Script-only
changes apply on the next hook call. Do not hand-edit hooks or copy trust hashes.

`python3 scripts/smoke_test.py` checks the installed commands with temporary
sessions and real audio, cleaning up only its own rows. Avoid `--restart` while
other sessions have pending rows. `scripts/test_codex_live.py` additionally uses
the configured model/account. Inspect `target/hook-events.jsonl` for decisions,
`target/desktop-notify.log` for service errors, and `/health` for API 4. Wire/native
changes require `python3 scripts/build.py` and a planned service restart; a
restart loses in-memory rows. A healthy service needs no restart for Python edits.

For explicitly requested task/sound management, use the web interface at `/` or
the documented API: `/acknowledge/{id}` quiets one row, `/dismiss/{id}` removes one,
`/sound/stop` quiets all while keeping rows, `/sound/snooze` mutes temporarily, and
`/stop` clears everything. Read current IDs before row actions. Never clear or
silence other sessions as test cleanup.
