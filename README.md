# desktop-notify

A local REST API for getting an agent user's attention, with looping sounds,
a Rust/Tauri tray app, and global Codex CLI and Claude Code hooks.

This Rust workspace starts with
[`agent-notify-server`](crates/agent_notify_server/README.md), copied from ArtCraft.
Additional crates can live under `crates/` and share dependencies declared in the
root `Cargo.toml`.

## Run

```sh
python3 scripts/build.py
target/debug/agent-notify-server
```

The server listens on `http://127.0.0.1:43110`. Open that address for the API
reference, or send requests directly:

```sh
curl -X POST http://127.0.0.1:43110/awaiting_user_input \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"agent-a","title":"Database migration","message":"Which database should I target?"}'

curl -X POST http://127.0.0.1:43110/all_tasks_finished \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"agent-b","title":"Login fix completed","message":"Fixed the expired-session redirect. All 12 tests pass."}'

curl -X POST http://127.0.0.1:43110/stop
curl http://127.0.0.1:43110/state
```

Press Ctrl+C to stop the server.

Open **http://127.0.0.1:43110/** in a browser for the web interface: live tasks
with Dismiss/Clear, the sound controls, and the full API reference. The tray
app's **API ↗** link opens it, and each app button's tooltip names its REST call.

The status endpoints (`/awaiting_user_input`, `/all_tasks_finished`,
`/task_failed`) require nonblank `title` (up to 200 characters) and `message` (up
to 4,000), and accept optional `session_id` (nonblank, up to 256 bytes, no
control characters), `context`, `origin`, and `agent`. They return `{id,
session_id?, state, title, message, context?, origin?, agent?}`. Each session has
one row: an update replaces only that session's row with a fresh `id` and its new
state. Requests without `session_id` share one unassigned row. Rows are ordered
by most recent update and live in memory until cleared or the server restarts.

- Every row has a `state`: `working` (busy, quiet), `input_needed`, `done`,
  `failed` (alerting), or the quiet `input_needed_ignored`, `done_acknowledged`,
  `failed_acknowledged`.
- `agent` is `claude_code` or `codex` (unrecognized values read back as
  `unknown`). A row keeps its agent when an update omits it, and the app shows a
  small Claude or Codex mark beside the state.
- `POST /working` marks a session busy (from a submitted prompt, or with
  `only_if_waiting` after the user answers).
- `POST /acknowledge/{id}` (Dismiss) quiets one alerting row, which stays listed.
  `POST /dismiss/{id}` (Clear) removes one row. Both return `{stopped: bool}`, and
  stale IDs are harmless.
- `GET /notifications` returns all rows, including quiet ones.
- `GET /sound` returns `{snoozed_until, alerting}` (an RFC 3339 UTC timestamp or `null`).
- `POST /sound/stop` quiets everything: every alerting row is acknowledged and any
  snooze is cancelled. Rows stay listed; later updates sound again. The
  `stop-sound` shell alias calls it.
- `POST /sound/snooze` with `{"seconds": 60}` (1 to 86,400) mutes all sound until
  that wall-clock time. Alerts arriving meanwhile stay quiet, then the loop
  resumes. Snoozing again replaces the deadline; `POST /sound/resume` ends it early.
- `POST /stop` clears **all** rows, any snooze, and all audio. It is the web
  interface's "Clear all tasks"; hooks never call it.
- `GET /state` includes `notifications`, `audio_notification_id`, `sound`, and
  desktop/audio status.

One shared sound loop plays for the newest `input_needed` row, else `failed`,
else `done`. Questions and failures use `alert_await_user_input_sound`;
completions use `alert_done_sound`. Dismissing or clearing the audible row moves
on to another outstanding alert; changes to other rows do not interrupt
playback. A snooze records only its end time. The service compares it with the
current time whenever state is read or changed (the tray app polls every 400 ms),
so no timer runs and sound resumes on the first poll after it elapses.

## Optional session context

Both notification endpoints accept a `context` object. Every field is optional:

```json
{
  "session_id": "agent-a",
  "title": "Which database should I target?",
  "message": "The migration is ready for your choice of database.",
  "context": {
    "cwd": "/workspace/customer-portal",
    "work_arc": "Modernize customer account management",
    "current_ask": "Add the account migration and verify existing users",
    "repo_name": "customer-portal",
    "repo_description": "The customer account and billing application."
  }
}
```

Context is returned by `/notifications` and `/state`. For a
named session, omitted or `null` fields retain their previous values; supplied
strings update them, and an empty string clears a field. A new `cwd` clears old
repo metadata unless replacements are also supplied. Unassigned entries do not
inherit context. The limits are 4,096 characters for `cwd`, 2,000 each for
`work_arc` and `current_ask`, 200 for `repo_name`, and 1,000 for `repo_description`.
Invalid context rejects the request without changing the existing row or audio.

The collapsed row shows a small project label (repo name, otherwise directory
basename), the alert title, and one ellipsized task line (current ask, otherwise
work arc). **Details** reveals the full message and available context. Empty
fields have no placeholders; notifications without context keep their simple
message layout. Hover over the project label for the full directory.

The Codex hook collects context locally, without executing repo code or making
model/network requests:

- `cwd` comes from the hook event. Repo metadata comes from the repository root's
  `package.json`, `Cargo.toml`, or `pyproject.toml`; the directory name and first
  README paragraph are fallbacks. Worktree `.git` files are supported.
- When the supplied `transcript_path` is readable, the current ask comes from the
  latest saved user request. The work arc uses that run's first agent progress
  summary or available plan explanation; an explicit `create_goal` objective
  takes precedence. These are excerpts of existing text, not generated summaries.
- Transcript parsing is best effort because Codex's transcript format is not a
  stable interface. At most the last 8 MiB are read, missing/unsupported context
  is omitted, and context failures never suppress the notification.
- Explicit `context` on the hook event overrides discovery. Callers can also set
  `NOTIFY_CWD`, `NOTIFY_WORK_ARC`, `NOTIFY_CURRENT_ASK`, `NOTIFY_REPO_NAME`, and
  `NOTIFY_REPO_DESCRIPTION`. Direct API callers can update context on every alert.

The three existing hook definitions are unchanged, so already-running sessions
use the new collection logic on their next question, approval, or completion.

## Global Codex CLI hooks

For how hooks, origin capture, and Focus fit together, including exact Ghostty
and tmux targeting and troubleshooting, see
[Connecting an agent to the server](AGENTS.md#connecting-an-agent-to-the-server).

```sh
python3 scripts/install_hooks.py            # Preview the exact hooks
python3 scripts/install_hooks.py --install  # Install, back up, trust, verify
python3 scripts/install_hooks.py --verify   # Ask Codex to verify enabled + trusted
```

The installer updates **`~/.codex/hooks.json`** and stores the exact definitions'
trust hashes in **`~/.codex/config.toml`** using Codex's configuration API. It replaces
the previous `afplay` notification hooks, preserves unrelated configuration, and
creates timestamped `*.desktop-notify-backup-*` files alongside both files. It does
not replace the existing top-level `notify` integration. Start a new Codex session
after installing so it loads the new hooks. The implementation is
[`scripts/codex_hook.py`](scripts/codex_hook.py); keep this checkout in place.

### Claude Code

```sh
python3 scripts/install_hooks.py --claude            # Preview
python3 scripts/install_hooks.py --claude --install  # Install with a backup
```

This adds the same Stop, PermissionRequest, and PreToolUse (`AskUserQuestion`)
hooks to **`~/.claude/settings.json`** (or `$CLAUDE_CONFIG_DIR`), preserving
unrelated settings. It removes older `~/.claude/agent_notify*.sh` hooks: those
called the now-removed sound-only `/loop_*` endpoints and `/stop`, which created
no status row and cleared every agent's row. Start a new Claude Code session afterwards. Stop rows
use Claude's final message (falling back to its transcript) and its latest prompt
as the current task.

| Codex event | API |
| --- | --- |
| `Stop` | `POST /all_tasks_finished`, with the final response as the outcome |
| `PermissionRequest` | `POST /awaiting_user_input`, with the approval description |
| `PreToolUse` matching `request_user_input` / `request_user_input_async` | `POST /awaiting_user_input`, with the actual questions |

Plain-text final questions are treated as awaiting input using a small heuristic;
structured question tools are more reliable. Titles include the working directory
name and the first line of the question/outcome. Long text is truncated to API
limits. A `Stop` event represents completion of a Codex turn, not all concurrent
sessions. Every question, approval, and completion request forwards Codex's
`session_id`, falling back to `CODEX_THREAD_ID` when an event has no ID. Two Codex
sessions in the same project therefore remain separate. Codex documents the field
in its [hook input reference](https://learn.chatgpt.com/docs/hooks#common-input-fields).
Subagent hooks use their parent's session ID; this integration tracks independent
Codex sessions. The existing hook commands load the updated Python script on every
invocation, so already-running sessions with these hooks pick up session tracking.

Every matching hook first checks `/health`. When the server is absent, a file lock
serializes concurrent launches, `cargo build --locked --workspace` rebuilds the Rust
server and Tauri app, and the server starts detached. Existing healthy servers are
reused; a closed tray app is reopened. Build/start output goes to
`target/desktop-notify.log`. Hook
failures appear as Codex warnings and never grant or deny a tool permission.

## Tauri tray app

The build creates and ad-hoc signs **`target/desktop/Desktop Notify.app`**. Its Rust
shell, dark frameless UI, and tray behavior follow the sibling Todo app. The
frontend is plain HTML/CSS/JavaScript bundled by Tauri; no Node build step is needed.

- New alerts open the window above other apps, on every Space, including full-screen apps.
- Each row shows its task state (**Working**, **Input needed**, **Finished**,
  **Failed**, or a dimmed "ignored"/"seen" variant), a short session ID, and its
  own **Focus**, **Dismiss** (alerting rows only), and **×** buttons. **Dismiss**
  acknowledges just that task; other tasks keep alerting. **×** removes the row.
  Available project/task context is compact; expand **Details** (or **View
  message**) for the full text. Only alerting rows open the window or mark the tray.
- Sound is global. The bar above the status line has **Stop sound** (`POST
  /sound/stop`, retaining every row), **Snooze 1 min**, and **Snooze 5 min**
  (`POST /sound/snooze`), with a countdown while snoozed. The footer's **API ↗**
  opens the web interface.
- **×** calls `POST /dismiss/{id}` and removes only that row. The window stays open
  while other entries remain; clearing the last entry hides it.
- **Hide to tray**, Escape, closing, or minimizing hides the window without
  dismissing the alert. Its sound keeps playing until stopped, snoozed, or cleared.
- Clicking the bell tray icon recalls the status list. Right-click opens the
  Show Notifications / Hide to Tray / Quit menu. A dot marks an active alert.
- When idle, the app stays in the tray. Recalling it shows **All caught up**.
- The app stays alive across service restarts and reconnects automatically. A
  hidden alert stays hidden until recalled or replaced with a new alert.

The Tauri Rust client talks directly to the local service. It polls every 400 ms
from `/notifications` and `/sound` and reports `/desktop/status` heartbeats. `/state` includes `desktop_connected`
and `desktop` presentation/window visibility/process details, including `displayed_ids`. The tray app defaults
to `http://127.0.0.1:43110`; the service passes its actual address with
`--server-url` when launching the app. Keep one service/app pair running.

Native Notification Center and the earlier Swift app have been replaced.

## Focus the requesting terminal

The **Focus** button beside an alert brings you back to its originating app on
macOS. It tries a terminal session, TTY, window ID, window title, then the app,
using whichever hints are available. A closed session or window falls back to
the broader target. Focus keeps the notification window visible so you can
interact with the terminal and click **Stop sound** or **×** later. It does not
dismiss the alert or stop its sound. Alerts without origin details still work
and show a disabled Focus button.

Both notification POST endpoints accept an optional `origin` object. Every field
inside it is optional, including the window and pane fields:

```json
{
  "title": "Database migration",
  "message": "Which database should I target?",
  "origin": {
    "terminal_app": "ghostty"
  }
}
```

| Optional field | Meaning |
| --- | --- |
| `terminal_app` | `ghostty`, `terminal`, `iterm2`, or a macOS app bundle ID |
| `app_pid` | Running GUI application's process ID |
| `pid` | Requesting process ID; the app walks its ancestry to find the GUI app |
| `terminal_id` | Ghostty AppleScript terminal ID or iTerm2 session UUID |
| `tty` | Terminal device such as `/dev/ttys001` (Terminal.app or iTerm2) |
| `window_id` | App's scripting window ID, encoded as a string |
| `window_title` | Exact window title; duplicate titles fall back to the app |
| `tmux_socket` | Absolute path to the tmux server socket |
| `tmux_pane` | Stable pane ID such as `%101` |
| `tmux_client` | Attached client's TTY; used when switching to a tmux pane |
| `tab_id` | Ghostty tab containing `terminal_id` (informational) |
| `tmux_server_pid` | tmux server process; Focus refuses a pane if the socket's server changed |
| `tmux_session`, `tmux_session_name` | Session ID such as `$3`, and its name |
| `tmux_window`, `tmux_window_index`, `tmux_window_name` | Window ID such as `@12`, its index, and its name |

Ghostty, Terminal.app, and iTerm2 have specific window/session adapters. Other
apps support application activation and exact window-title matching through
Accessibility. Focus only activates running apps. It reports failure if the app
has exited. Without an app identity, session/window hints are searched among
the three supported terminals; an ambiguous app fallback is not guessed.

The Codex hook automatically captures process and terminal hints. Inside tmux,
it uses the most recently active client attached to the originating session to
find the GUI app, and records the pane/socket when available. Selecting a tmux
pane is best effort and requires its socket; failure still permits app focus.
Ghostty 1.3 exposes neither its AppleScript terminal ID nor a TTY through scripting,
so the hook identifies the surface itself: it briefly sets the outer TTY's title
(the attached tmux client's TTY, bypassing tmux) to a random marker, asks Ghostty
which terminal carries it, and restores the previous title. The window, tab, and
terminal IDs are cached per Ghostty process and TTY in `target/terminal-origins.json`;
later alerts only confirm the cached terminal still exists. The lookup runs a
fixed AppleScript with a 3-second limit and only when Ghostty hosts the session;
depending on what launched the agent, macOS may ask once to allow it to control
Ghostty. Set `NOTIFY_GHOSTTY_PROBE=0` to disable it. Inside tmux the hook also
reports the server PID, session, and window. Callers that know precise
IDs can pass them in the API or set `NOTIFY_TERMINAL_ID`, `NOTIFY_WINDOW_ID`,
`NOTIFY_WINDOW_TITLE`, `NOTIFY_TERMINAL_APP`, or `NOTIFY_TTY` before launching Codex.
Do not use Ghostty's newer core surface ID as an AppleScript terminal ID.

If the probe cannot run (for example, a fixed `title` in the Ghostty configuration),
you can still associate a Ghostty window with its shell manually, including when it
is on another Space, by running this from a foreground shell in that window:

```sh
python3 /path/to/desktop-notify/scripts/register_terminal.py --test
```

This records the window ID for the outer terminal's TTY and Ghostty process.
With tmux, another pane in the same attached terminal is fine. Future hook
notifications use this registration automatically; no hook reinstall is needed.
Registrations live in ignored `target/terminal-origins.json`. Re-register after
restarting Ghostty or opening a new terminal. Add `--terminal` to also register
the selected Ghostty terminal/pane; the default only registers the window.
The optional `--test` waits ten seconds so you can switch Spaces, then sends an
alert with the exact window ID and no tmux targeting. It clears that test after
60 seconds without dismissing any replacement notification.

Window/session scripting may prompt for macOS **Automation** permission for
Desktop Notify. Generic window-title matching additionally needs Accessibility.
Application-only activation needs neither. Origin values are validated and passed
as arguments to fixed scripts; they are never executed as shell or AppleScript code.

## Configuration

Bundled sounds and the default YAML configuration live in
[`crates/agent_notify_server/config/`](crates/agent_notify_server/config/notify_config.yaml).
Sound paths are resolved relative to the YAML file. The default configuration path
is embedded at build time, so running from another directory works as long as the
source configuration and sounds remain in place. When moving the binary to another
machine, also copy the configuration and sounds and set `NOTIFY_CONFIG_PATH`.

Environment variables:

- `HTTP_BIND_ADDRESS`: listener address (default `127.0.0.1:43110`).
- `NOTIFY_CONFIG_PATH`: path to a custom YAML configuration.
- `RUST_LOG`: logging filter (default `info,actix_web=info`).
- `NOTIFY_APP_PATH`: alternate path to the built macOS app bundle.
- `NOTIFY_DISABLE_DESKTOP`: set to skip launching the Tauri app (e.g. headless tests).

See the [server README](crates/agent_notify_server/README.md) for the full API,
loop escalation settings, and agent integration examples.

## Development

```sh
cargo fmt --all --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
python3 -m unittest discover -s scripts -p 'test_*.py'
node --test scripts/test_ui.mjs
python3 scripts/smoke_test.py --restart  # Real hooks + audio + concurrent cold start
python3 scripts/test_codex_live.py      # Optional: one real Codex question/answer turn
```

The lockfile pins both the audio server and Tauri dependencies.

Playback requires an audio output device. Linux builds also need ALSA development
headers and `pkg-config` (for example, `libasound2-dev` and `pkg-config` on Debian or
Ubuntu).

The desktop app additionally needs Tauri's platform prerequisites. macOS uses
Xcode Command Line Tools; Linux needs WebKitGTK 4.1 and AppIndicator development
libraries. The server can still be built alone with `cargo build -p agent-notify-server`.

The smoke test plays both sounds briefly and stops them in a `finally` block. The
live test uses your configured model/account and verifies actual Codex hook
dispatch. Both require the global hooks to have been installed. The API is intended
for trusted local callers; keep the default loopback bind address.
