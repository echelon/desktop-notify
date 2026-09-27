# Desktop Notify

## Project and layout

Desktop Notify is a local agent attention service: a Rust REST API owns looping
audio and per-session notifications, a Tauri tray app displays them, and Python
hooks forward Codex and Claude Code questions, permission requests, and completed
turns. The
desktop experience is primarily macOS. The service defaults to
`http://127.0.0.1:43110` and is intended for trusted local callers.

| Path | Responsibility and main libraries |
| --- | --- |
| `crates/agent_notify_server/` | Actix Web + Tokio HTTP server, rodio/Symphonia audio, Serde YAML configuration, log/env_logger, anyhow, rand. |
| `crates/notify_types/` | Shared Serde notification, origin, and context types and validation. |
| `crates/desktop_notify_app/` | Tauri 2 tray/window shell, reqwest service client, objc2/AppKit integration, fixed AppleScript focus adapters. |
| `ui/` | Plain HTML/CSS/JavaScript bundled directly into Tauri. |
| `scripts/` | Python standard-library build, hook, origin/context discovery, installation, and test helpers; Node's built-in UI tests. |

Each of these directories has scoped `AGENTS.md` guidance. Read it before editing
that area. `README.md` describes the current user-facing behavior; the server
README and `crates/agent_notify_server/static/index.html` describe the API.

## Coding conventions

- **Rust uses two spaces**, Rust 2021, and Unix newlines. `rustfmt.toml` is the
  formatting authority; use `cargo fmt --all` for Rust edits.
- Follow existing naming: Rust/Python `snake_case`, Rust types `PascalCase`,
  JavaScript `camelCase`. Python uses four spaces; JavaScript uses two.
- Keep shared Rust dependencies in `[workspace.dependencies]` and inherit them
  in member crates. Respect the pinned Tauri versions and committed `Cargo.lock`;
  use `--locked` for normal builds/checks/tests.
- Keep the frontend free of a framework or bundler and Python scripts free of
  third-party dependencies unless the task calls for changing that architecture.
- Keep HTTP, audio, presentation, and hook responsibilities in their existing
  layers. Put shared wire types in `notify-types` instead of duplicating structs.
- Handle malformed input and unavailable services with useful errors. Optional
  context or focus discovery must not prevent the primary notification.
- Explain concurrency, compatibility, and platform constraints in comments.
  Keep macOS APIs behind `cfg(target_os = "macos")`.

## Behavior learned while building

- Independent Codex sessions must remain independent, even in the same directory.
  A session ID identifies a row; a fresh notification ID identifies each update.
  Stale row actions must never affect a replacement or another session.
- Focus, Stop sound, Clear, and Hide have separate meanings. Focus leaves the row
  and sound intact; Stop sound is global and silences every row but retains them;
  Clear removes that row; Hide only changes window visibility. `/stop`
  intentionally clears everything.
- Snooze mutes all sound until a recorded wall-clock deadline (chrono
  `DateTime<Utc>`), checked against the current time on requests; never a timer.
- Pending questions have sound priority over completions. Preserve one shared
  audio loop and do not restart it for unrelated row changes.
- The persistent, always-on-top Tauri tray window replaced the Swift/Notification
  Center experiment. Preserve tray recall, all-Spaces/full-screen visibility,
  and reconnection after service restarts.
- Treat notification text and focus hints as data: render text literally and pass
  process arguments separately. Never interpolate them into executable code.
- Preserve legacy endpoints and optional-field compatibility when extending the
  API. Update producers, shared types, consumers, tests, and API docs together.

## Connecting an agent to the server

### Setup

Agents talk to the server only through `scripts/codex_hook.py`. Install it with
the installer, never by hand, then start a **new** agent session (running
sessions keep the hook definitions they started with):

```sh
python3 scripts/install_hooks.py --install           # Codex: ~/.codex/hooks.json + trust hashes
python3 scripts/install_hooks.py --claude --install  # Claude Code: ~/.claude/settings.json
```

Omit `--install` to preview. Both write backups and preserve unrelated hooks and
settings, and both register the same three hooks with the same command
(`<python> <repo>/scripts/codex_hook.py`), synchronous with a 720 s timeout so a
cold build/start can finish:

| Agent event | Endpoint | Row kind |
| --- | --- | --- |
| Stop (turn finished) | `/all_tasks_finished`, or `/awaiting_user_input` if the final message asks a question | Finished / Input needed |
| PermissionRequest | `/awaiting_user_input` | Input needed |
| PreToolUse matching `(^\|.*[._])(request_user_input(_async)?\|AskUserQuestion)$` | `/awaiting_user_input` | Input needed |

Never wire an agent to the legacy sound-only endpoints (`/loop_*`, `/alert_*`) or
call `/stop` from a hook. They create no row, and `/stop` or `/loop_*` clear
**every** session's row. The Claude installer removes old
`~/.claude/agent_notify*.sh` hooks that did this. Editing `scripts/*.py` needs no
reinstall because hooks load the script on each run. Changing the Rust wire types
(`notify-types`) does need `python3 scripts/build.py` and a service restart: the
server rejects unknown origin fields, so an old server returns 400 for new hooks.
A restart discards in-memory rows, so re-post any pending ones.

### What a hook sends

`codex_hook.py` reads the hook JSON on stdin (Codex and Claude Code use the same
field names) and posts `{title, message, session_id, context, origin}`:

- `session_id` comes from the event and identifies the row, one per agent session.
- `message`: the event's `last_assistant_message` for Stop. Claude Code may omit
  it, in which case the hook uses the newest assistant text in `transcript_path`.
- `context` (`notification_context.py`) holds the cwd, repo name/description
  from manifests or README, and `current_ask`, taken from the latest real user
  prompt in either transcript format.
- `origin` (`notification_origin.py`) holds focus hints, described below. Context
  and origin are best-effort; their failure never suppresses the alert.

### How Focus finds the exact terminal

Capture happens in the hook, at alert time:

1. **Process and app:** walk the hook's process ancestry to the `codex`/`claude`
   process (`pid`). Inside tmux, the server is daemonized and its ancestry does
   not reach the GUI. Instead, ask the tmux server named by `$TMUX`, using its
   socket, for the most recently active client attached to this pane's session.
   That client's TTY is the outer terminal (`tty`, `tmux_client`), and its
   ancestry leads to the GUI app (`terminal_app` bundle ID, `app_pid`).
2. **tmux location:** from `$TMUX`/`$TMUX_PANE`, record `tmux_socket`, `tmux_pane`
   (`%N`), `tmux_server_pid`, `tmux_session` (`$N`), `tmux_window` (`@N`),
   `tmux_window_index`, and the session/window names.
3. **Ghostty surface:** Ghostty's AppleScript API has window/tab/terminal IDs
   but no TTY or PID, so the hook names the surface itself (`probe_ghostty`).
   It snapshots all terminal titles, writes a random OSC 2 title directly to the
   outer TTY (bypassing tmux), and asks the fixed read-only
   `scripts/ghostty_surface.applescript` which terminal carries the marker. It
   then restores the old title as printable text and records `window_id`,
   `tab_id`, and `terminal_id`. The result is cached in ignored
   `target/terminal-origins.json` under `<app_pid>:<tty>`. Later alerts only
   confirm the cached terminal still exists, since a live surface keeps its PTY.
   A missing terminal triggers a fresh probe. Manual `register_terminal.py`
   pairings are the fallback, and `NOTIFY_GHOSTTY_PROBE=0` disables the probe.
   Other terminals use their own hints: iTerm2's `ITERM_SESSION_ID`, and TTYs
   for Terminal.app/iTerm2.

Focus happens in the desktop app (`crates/desktop_notify_app/src/focus.rs`),
when the row's Focus button is clicked:

1. **tmux first.** Resolve `tmux_pane` on `tmux_socket`, and refuse it if the pane
   is gone or `#{pid}` differs from `tmux_server_pid` (pane IDs restart with a
   new server). Then `switch-client -c <tmux_client> -t <pane>` and
   `select-pane`, which change session, window, and pane even if you have moved
   elsewhere.
2. **Then the window,** trying `terminal_id`, then `tty`, then `window_id`, then
   `window_title`, then the app itself, through the fixed `src/focus/*.applescript`
   adapters. Arguments go through argv, never interpolated. For Ghostty,
   `focus <terminal>` raises that surface's window and tab, and macOS switches
   to its Space or full-screen space. It works across Ghostty windows and tmux
   clients.
3. A tmux failure still focuses the window, with a warning on the row. Focus
   never hides, silences, or dismisses the row.

Rows captured before a capture improvement keep their old, weaker origin. They
become exact at the session's next hook event.

### Checking and troubleshooting

- **Check a hook:** pipe a Stop event into the installed command, then inspect
  the row's `origin`:
  `echo '{"hook_event_name":"Stop","session_id":"test","cwd":"'$PWD'","last_assistant_message":"hi"}' | <python> scripts/codex_hook.py`,
  then `curl -s 127.0.0.1:43110/notifications`. Dismiss the test row afterwards
  with `POST /dismiss/<id>`.
- **Check capture alone:** run
  `cd scripts && python3 -c 'import notification_origin as o; print(o.capture_origin())'`
  from the terminal you want identified. Expect `terminal_id` for Ghostty and
  `tmux_*` fields inside tmux.
- **No row appears:** make sure the hook is installed for this agent and the
  session was started after installing. Check `target/desktop-notify.log` and
  that `/health` answers.
- **Focus reaches Ghostty but not the window:** the row predates the fix, the
  probe was disabled, a fixed Ghostty `title` config ignores OSC 2, or macOS
  denied the process permission to control Ghostty. Allow it in System Settings →
  Privacy & Security → Automation, or run `scripts/register_terminal.py` in that
  window.
- **Unit tests** must patch `notification_origin.ghostty` and TTY writes. Never
  script the real Ghostty or write titles to real TTYs from tests.

## Build and verification

Run commands from the repository root:

```sh
python3 scripts/build.py
cargo fmt --all --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
python3 -m unittest discover -s scripts -p 'test_*.py'
node --test scripts/test_ui.mjs
```

`build.py` builds the workspace and assembles/ad-hoc signs
`target/desktop/Desktop Notify.app` on macOS. Run it when validating bundled UI
or native behavior. The service alone can be built with
`cargo build --locked -p agent-notify-server`. Linux audio needs ALSA development
headers and pkg-config; the desktop additionally needs Tauri platform libraries.

Choose checks for the changed area. Add regression coverage for behavior changes,
especially replacement races, session isolation, optional data, and focus fallback.
For documentation-only edits, check paths, links, and the diff; no runtime launch
is needed. Unit tests use recorded audio commands/mocks; real audio, Spaces,
Automation, and tray behavior need a live check when affected.

`python3 scripts/smoke_test.py` exercises installed hooks and plays real audio;
`--restart` additionally restarts the service and clears its in-memory state.
`python3 scripts/test_codex_live.py` uses the configured Codex model/account.
Use these deliberately for relevant integration changes and report what ran.

## Repository hygiene

- Keep generated bundles, logs, terminal registrations, caches, and Tauri schemas
  in their existing ignored locations. Do not commit `target/` or local hook config.
- Keep rule content in `AGENTS.md`. Every rule file must have a sibling relative
  symlink `CLAUDE.md -> AGENTS.md`; do not maintain separate copies.
