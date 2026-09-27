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

Agents talk to the server only through `scripts/codex_hook.py`, which posts rows to
`POST /awaiting_user_input` and `POST /all_tasks_finished` with the event's
`session_id`, collected `context`, and `origin`. Install it with the installer,
not by hand, and start a new agent session afterwards:

```sh
python3 scripts/install_hooks.py --install           # Codex: ~/.codex/hooks.json + trust
python3 scripts/install_hooks.py --claude --install  # Claude Code: ~/.claude/settings.json
```

Omit `--install` to preview. Both register the same three hooks: Stop,
PermissionRequest, and PreToolUse matching `request_user_input(_async)` or
`AskUserQuestion`, each synchronous with a 720 s timeout for cold build/start.
Never wire an agent to the legacy sound-only endpoints (`/loop_*`, `/alert_*`) or
call `/stop` from a hook: they create no row and `/stop`/`/loop_*` clear **every**
session's row. The Claude installer removes old `~/.claude/agent_notify*.sh` hooks
that did this. To check an install, pipe a Stop event into the installed command
and confirm a row appears in `GET /notifications`.

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
