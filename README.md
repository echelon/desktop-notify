# desktop-notify

A local REST API for getting an agent user's attention, with looping sounds,
a Rust/Tauri tray app, and global Codex CLI hooks.

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
  -d '{"title":"Database migration","message":"Which database should I target?"}'

curl -X POST http://127.0.0.1:43110/all_tasks_finished \
  -H 'Content-Type: application/json' \
  -d '{"title":"Login fix completed","message":"Fixed the expired-session redirect. All 12 tests pass."}'

curl -X POST http://127.0.0.1:43110/stop
curl http://127.0.0.1:43110/state
```

Press Ctrl+C to stop the server.

Both POST endpoints require nonblank `title` (up to 200 characters) and `message`
(up to 4,000 characters). They return `{id, kind, title, message}` and replace the
previous alert. Awaiting input uses `alert_await_user_input_sound`; completion uses
`alert_done_sound`. Sound continues until dismissed, stopped, or replaced.
`POST /dismiss/{id}` dismisses only that alert; an old notification cannot silence
a newer one. `GET /stop` and all legacy sound endpoints remain available.

## Global Codex CLI hooks

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

| Codex event | API |
| --- | --- |
| `Stop` | `POST /all_tasks_finished`, with the final response as the outcome |
| `PermissionRequest` | `POST /awaiting_user_input`, with the approval description |
| `PreToolUse` matching `request_user_input` / `request_user_input_async` | `POST /awaiting_user_input`, with the actual questions |

Plain-text final questions are treated as awaiting input using a small heuristic;
structured question tools are more reliable. Titles include the working directory
name and the first line of the question/outcome. Long text is truncated to API
limits. A `Stop` event represents completion of a Codex turn, not all concurrent
sessions. This server intentionally has one active alert, with the latest winning.

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
- **Dismiss & Stop** (or Enter) calls `POST /dismiss/{id}`, stops the matching sound,
  and hides the window once the service confirms the alert has cleared.
- **Hide to tray**, Escape, closing, or minimizing hides the window without
  dismissing the alert. Its sound keeps playing until explicitly dismissed.
- Clicking the bell tray icon recalls the current alert. Right-click opens the
  Show Notifications / Hide to Tray / Quit menu. A dot marks an active alert.
- When idle, the app stays in the tray. Recalling it shows **All caught up**.
- The app stays alive across service restarts and reconnects automatically. A
  hidden alert stays hidden until recalled or replaced with a new alert.

The Tauri Rust client talks directly to the local service. It polls every 400 ms
and reports `/desktop/status` heartbeats. `/state` includes `desktop_connected`
and `desktop` presentation/window visibility/process details. The tray app defaults
to `http://127.0.0.1:43110`; the service passes its actual address with
`--server-url` when launching the app. Keep one service/app pair running.

Native Notification Center and the earlier Swift app have been replaced.

## Focus the requesting terminal

The **Focus** button beside an alert brings you back to its originating app on
macOS. It tries a terminal session, TTY, window ID, window title, then the app,
using whichever hints are available. A closed session or window falls back to
the broader target. Focus hides the notification window after success; it does
not dismiss the alert or stop its sound. Alerts without origin details still
work and show a disabled Focus button.

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

Ghostty, Terminal.app, and iTerm2 have specific window/session adapters. Other
apps support application activation and exact window-title matching through
Accessibility. Focus only activates running apps. It reports failure if the app
has exited. Without an app identity, session/window hints are searched among
the three supported terminals; an ambiguous app fallback is not guessed.

The Codex hook automatically captures process and terminal hints. Inside tmux,
it uses the most recently active client attached to the originating session to
find the GUI app, and records the pane/socket when available. Selecting a tmux
pane is best effort and requires its socket; failure still permits app focus.
Ghostty 1.3 does not expose its AppleScript terminal ID in the shell environment,
so automatic capture can fall back to Ghostty itself. Callers that know precise
IDs can pass them in the API or set `NOTIFY_TERMINAL_ID`, `NOTIFY_WINDOW_ID`,
`NOTIFY_WINDOW_TITLE`, `NOTIFY_TERMINAL_APP`, or `NOTIFY_TTY` before launching Codex.
Do not use Ghostty's newer core surface ID as an AppleScript terminal ID.

To associate a specific Ghostty window with its shell (including when it is on
another Space), run this from a foreground shell in that window:

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
