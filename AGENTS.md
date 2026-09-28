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

## Invariants learned while building

- Independent agent sessions must remain independent, even in the same
  directory. A session ID identifies a row; a fresh notification ID identifies
  each update. Stale row actions must never affect a replacement or another session.
- Focus, Dismiss, Stop sound, Clear, and Hide have separate meanings (see
  Features). `/stop` intentionally clears everything.
- One shared audio loop. Do not restart it for unrelated row changes. Snooze is a
  recorded wall-clock deadline (chrono `DateTime<Utc>`), never a timer.
- The persistent, always-on-top Tauri tray window replaced the Swift/Notification
  Center experiment. Preserve tray recall, all-Spaces/full-screen visibility,
  and reconnection after service restarts.
- The tray app is a stateless client of the HTTP API. It caches only the last
  poll (to render, to open the window for new alerting IDs, to stay visible
  offline, and to find Focus targets) plus transient UI state. Never give it task
  state or timers of its own; add them to the service and read them.
- The service's in-memory state is the only source of truth. The `/tmp` task
  backup is a convenience seed for the next process, never authoritative, and
  must never block startup, requests, or shutdown.
- Treat notification text and focus hints as data: render text literally and pass
  process arguments separately. Never interpolate them into executable code.
- Preserve legacy endpoints and optional-field compatibility when extending the
  API. Update producers, shared types, consumers, tests, and API docs together
  (including the web interface at `GET /`). Before removing an endpoint, check
  every caller: both agents' hooks, the app, the web interface, and the user's
  shell aliases.

## Task states

Every row has one `TaskState` (`crates/notify_types`, serialized snake_case).
Only **alerting** states feed the sound loop, open the window, and set the tray
dot. The row's `state` field is its only status.

| State | Alerting | Entered by | Dismiss moves it to |
| --- | --- | --- | --- |
| `working` | no, busy | `POST /working` (prompt submitted, or resumed after input) | — |
| `input_needed` | **yes**, await sound | `POST /awaiting_user_input` (question, permission request) | `input_needed_ignored` |
| `input_needed_ignored` | no, still waiting | Dismiss or Stop sound on `input_needed` | — |
| `done` | **yes**, done sound | `POST /all_tasks_finished` (turn finished) | `done_acknowledged` |
| `done_acknowledged` | no, terminal | Dismiss or Stop sound on `done` | — |
| `failed` | **yes**, await sound | `POST /task_failed` (turn ended on an error) | `failed_acknowledged` |
| `failed_acknowledged` | no, terminal | Dismiss or Stop sound on `failed` | — |

Sound priority is the newest `input_needed`, then the newest `failed`, then the
newest `done`. A new agent update to a session replaces its row with a fresh
ID, whatever the previous state. Candidate future states (not implemented): an
`ended` state when the agent's PID exits without Stop, and a stale-`working`
marker.

## HTTP API (current, API version 4)

Local JSON over HTTP at `http://127.0.0.1:43110` (`HTTP_BIND_ADDRESS` overrides).
Invalid input returns 400 with a plain-text reason and leaves state unchanged.
A missing configured sound returns 503.

**Reporting status** (used by hooks):

| Endpoint | Body | Effect |
| --- | --- | --- |
| `POST /awaiting_user_input` | `{title, message, session_id?, context?, origin?, agent?, tool_use_id?, turn_started_at?}` | Row becomes `input_needed`. `tool_use_id` names the tool call it waits on. |
| `POST /all_tasks_finished` | `{title, message, session_id?, context?, origin?, agent?, turn_started_at?}` | Row becomes `done`. |
| `POST /task_failed` | same as above | Row becomes `failed`. |
| `POST /working` | `{session_id, title?, message?, context?, origin?, agent?, only_if_waiting?, tool_use_id?}` | Row becomes `working` and returns `{updated, notification?}`. With `only_if_waiting`, it only resumes an `input_needed*` row waiting on the same `tool_use_id` (or either is unknown), and never creates a row or revives a finished one. Missing title/message keep the row's text. |

These return the row: `{id, session_id?, state, title, message, context?,
origin?, agent?, times?}`. Limits: title 1–200 characters, message 1–4000,
`session_id` and `tool_use_id` 1–256 bytes with no control characters. Rows are
keyed by `session_id`; omitted IDs share one legacy "unassigned" row. Named
sessions keep `context`, `origin`, and `agent` when an update omits them.
`context` fields (`cwd`, `work_arc`, `current_ask`, `repo_name`,
`repo_description`) merge: omitted means unchanged, and a blank string clears.
`agent` is `claude_code` or `codex`; other values read back as `unknown`.
`origin` fields are listed under Focus below.

`times` holds service-assigned timestamps (chrono `DateTime<Utc>`, RFC 3339 on
the wire). Every field is optional and omitted when the service did not observe
that moment; requests cannot set them (an alert's `turn_started_at` is only a
guarded fallback for an unseen task start). Only named sessions carry them across
replacements; the unassigned row starts fresh each time.

| Field | Set | Cleared or kept |
| --- | --- | --- |
| `tracked_since` | First report for the session | Kept until the row is cleared |
| `updated_at` | Every agent report, including an `only_if_waiting` call that changed nothing | Dismissals do not touch it |
| `task_started_at` | `/working` without `only_if_waiting`, unless the row is already `working` (a queued prompt joins the turn). If still unknown, an alert's `turn_started_at` fills it, unless that is in the future or not after the session's previous `task_finished_at` | Kept through `input_needed*`, resume, and `done`/`failed`; otherwise `None` |
| `task_finished_at` | `done`/`failed` | `None` for every other state |
| `waiting_since` | Entering `input_needed` (kept if already waiting) | `None` outside `input_needed*` |
| `dismissed_at` | Dismiss or Stop sound | Any agent update |
| `last_request_at` | Any request that touched the row: agent reports (including no-op ones), Dismiss, Stop sound (rows it quieted), Focus | Kept until the next one |
| `user_action_at` | Dismiss, Stop sound (rows it quieted), or Focus from the app or web interface | Kept across agent updates |
| `user_input_at` | Every move to `working`: the hooks send that only for a submitted prompt or an answered question/permission | Kept across agent updates; a finished tool that changed nothing does not set it |

Terminal activity is inferred only from those hook events. The service does not
watch TTYs or keystrokes.

The app's timing line (and the web interface's) is derived from these: running
for, waiting for, ran for, and finished/failed "N minutes ago".

**Acting on rows** (used by the app):

| Endpoint | Effect |
| --- | --- |
| `POST /acknowledge/{id}` | Dismiss: an alerting row moves to its quiet state and stays listed. Returns `{stopped}`; stale or already-quiet IDs return `false`. |
| `POST /dismiss/{id}` | Clear: remove that row. Returns `{stopped}`; stale IDs are harmless. |
| `POST /focused/{id}` | Record that the app focused this row's terminal (`user_action_at`). Changes nothing else. Returns `{recorded}`; stale IDs return `false`. |
| `POST /stop` | Clear every row, cancel any snooze, and stop all audio. Hooks must never call it. |

**Global sound** (used by the app's sound bar):

| Endpoint | Effect |
| --- | --- |
| `GET /sound` | `{snoozed_until, alerting}`; `snoozed_until` is RFC 3339 UTC or null. |
| `POST /sound/stop` (also `GET`) | Stop sound: acknowledge every alerting row and cancel any snooze. Rows stay. |
| `GET /stop` | Legacy `stop-sound` alias: identical to `GET /sound/stop`; rows and Focus targets stay. |
| `POST /sound/snooze` | `{seconds}` (1–86400): mute until now + seconds. Replaces an earlier snooze. |
| `POST /sound/resume` | End a snooze early. |

**Reading and service** (reads never change playback, except that noticing an
elapsed snooze resumes sound):

| Endpoint | Returns |
| --- | --- |
| `GET /notifications` | All rows, newest update first, including quiet ones. The app polls it every 400 ms. |
| `GET /state` | `{notifications, audio_notification_id, sound, desktop, desktop_connected, audio, config}`. |
| `GET /health` | `{service: "desktop-notify", api_version: 4, pid}`. Hooks check it before posting. |
| `POST /desktop/status` | App heartbeat `{presentation, window_visible, pid, displayed_id, displayed_ids, error}`. |
| `GET /` | **Permanent.** The web interface (`crates/agent_notify_server/static/index.html`): live tasks with Dismiss/Clear, sound controls (Stop, Snooze, Resume, Clear all), and this API reference. |

**Who calls what** (C = Claude Code hook, X = Codex hook, A = Tauri app,
W = web interface at `GET /`, S = `stop-sound` shell alias in
`~/.config/shell/aliases/20-shortcuts.sh`):

| Endpoint | Callers |
| --- | --- |
| `GET /health` | C, X |
| `GET /state` | C, X (checks the app is connected before reopening it) |
| `POST /awaiting_user_input`, `/all_tasks_finished`, `/working` | C, X |
| `POST /task_failed` | C (StopFailure) |
| `GET /notifications`, `GET /sound` | A, W (polling) |
| `POST /acknowledge/{id}`, `/dismiss/{id}`, `/sound/snooze` | A, W (buttons) |
| `POST /focused/{id}` | A (after the Focus button, best-effort) |
| `POST /sound/stop` | A, W, S (`curl -fsS -m 1 -X POST http://127.0.0.1:43110/sound/stop`) |
| `POST /sound/resume` | W |
| `POST /stop` | W ("Clear all tasks") |
| `GET /stop` | shells still holding the old `stop-sound` alias |
| `POST /desktop/status` | A (heartbeat) |
| `GET /` | you, in a browser; the app's **Web API ↗** link opens it |

API 4 added task states, `/working`, `/task_failed`, `/acknowledge`, and `agent`.
Hooks skip or adapt new calls on older services (`codex_hook.compatible`).

## Permanent, retained, and removed API

- **Permanent:** `GET /` is the web interface for managing tasks, alerts, and
  sound, and the live API reference. Never remove it; keep it in sync with the API.
- **Retained:** `GET /stop` serves shells that still hold the old `stop-sound`
  alias. It uses the same sound-only handler as `POST /sound/stop` and must never
  clear rows. `POST /stop` is the web interface's "Clear all tasks".
- **Removed** (API 4, after confirming no hook, app, alias, or Codex config
  called them): the sound-only `GET /alert_beep|alert_done|alert_await` and
  `GET /loop_beep|loop_done|loop_await` endpoints (the loops cleared every row),
  `GET /notification` (use `/notifications`), `POST /silence/{id}` (use
  `/acknowledge/{id}`), `POST /sound/silence` (renamed `/sound/stop`), the row
  fields `kind`/`silenced` (use `state`), `/state`'s `notification` field, the
  `alert_beep_sound`/`extra_alert_beep_sounds` config, and the audio engine's
  one-shot sink. Do not reintroduce them.

## Hooks: how agents report status

### Design

Agents never call the API directly. Codex and Claude Code run a **hook
command** on lifecycle events. The command is the same script for both,
`scripts/codex_hook.py --agent <codex|claude_code>`. It reads the event JSON on
stdin (both agents use the same field names), decides the endpoint, adds
context and origin, and posts to the API. It always prints `{}` (or a
`systemMessage` on failure) and never grants, denies, or blocks anything.

| Agent event | Endpoint | Resulting state |
| --- | --- | --- |
| UserPromptSubmit | `/working` with the prompt as title/message | `working` |
| PermissionRequest | `/awaiting_user_input` (+ `tool_use_id`) | `input_needed` |
| PreToolUse matching `(^\|.*[._])(request_user_input(_async)?\|AskUserQuestion)$` | `/awaiting_user_input` (+ `tool_use_id`) | `input_needed` |
| PostToolUse (except `request_user_input_async`'s immediate return) | `/working` with `only_if_waiting` (+ `tool_use_id`) | `input_needed*` → `working`, else no change |
| Stop (turn finished) | `/all_tasks_finished`, or `/awaiting_user_input` only if the message's closing paragraph asks a question or requests input | `done` / `input_needed` |
| StopFailure (Claude Code only; Codex has no such event) | `/task_failed` | `failed` |

Principles:

- **Alerts start the service** (build and launch if needed, under a lock so
  concurrent hooks start one), with a 720 s timeout. **Busy updates never do.**
  UserPromptSubmit (30 s) and PostToolUse (10 s) skip silently if the service is
  down or older than API 4, so they never block a prompt or tool.
- **PostToolUse is minimal and synchronous.** It sends only
  `{session_id, only_if_waiting, tool_use_id}`, without discovery, in about 65 ms.
  An asynchronous call could land after a newer question and cancel it. Matching
  `tool_use_id` stops a parallel tool from cancelling another tool's prompt.
- **An async question returning is not an answer.** Ignore its PostToolUse event;
  the next UserPromptSubmit or final status updates the row. Blocking questions
  still resume on their matching PostToolUse.
- **A finished turn is `done`**, even when work continues in the background.
  Report the background result as a new status when it completes. Put any real
  question in the closing paragraph, because Stop classification reads only that.
- **Agent identity is self-reported.** The installer writes `--agent <name>` into
  each agent's command. The fallbacks are `NOTIFY_AGENT`, then the agent's
  environment (`CLAUDECODE`, `AI_AGENT`, `CODEX_THREAD_ID`), then the transcript
  location.
- **What a hook sends:** `session_id` from the event. `message` is Stop's
  `last_assistant_message`; if Claude omits it, the hook uses the newest assistant
  text in `transcript_path`. `context` comes from `notification_context.py`: cwd,
  repo metadata from manifests or README, and `current_ask` from the latest real
  prompt in either transcript format. `origin` comes from `notification_origin.py`
  (see Focus). Alerts add `turn_started_at` from `notification_context.turn_started_at`:
  Codex's latest `task_started` event, or Claude Code's latest typed prompt when
  no `stop_hook_summary`/`turn_duration` marker follows it (a turn without its
  own prompt reports nothing rather than an earlier turn's start). Context,
  origin, and turn start are best-effort; their failure never suppresses the alert.

### Setup

Install with the installer, never by hand, then start **new** agent sessions
(running sessions keep the hook definitions they started with):

```sh
python3 scripts/install_hooks.py --install           # Codex: ~/.codex/hooks.json + trust hashes
python3 scripts/install_hooks.py --claude --install  # Claude Code: ~/.claude/settings.json
```

For persistent Codex guidance, add `--with-guidance` to preview, install, or verify.
The installer expands `docs/codex/AGENTS.md` into a managed section of
`$CODEX_HOME/AGENTS.md`, preserving unrelated instructions and backing up existing
content, with `CLAUDE.md -> AGENTS.md` beside it. Edit the repository source and
reinstall; do not maintain a separate personal copy of the rules.

Omit `--install` to preview. Both back up and preserve unrelated hooks and
settings, replace earlier versions of this hook in place, and remove old
`~/.claude/agent_notify*.sh` hooks that called the removed endpoints. Codex
installation also registers the exact definitions' trust hashes through Codex's
config API and verifies them (`--verify`). Editing `scripts/*.py` needs no
reinstall, because hooks load the script on each run. Changing wire types
(`notify-types`) needs `python3 scripts/build.py` and a service restart, because
the server rejects unknown fields. A clean restart (SIGTERM/SIGINT) restores rows
from the task backup (see Features); after SIGKILL, rows from the last periodic
backup return. Never re-post a row by hand as a `/working` prompt: that resets
its task start.

## Features

- **Rows:** one per session, newest first, showing the state label, the agent
  mark (orange Claude Code mascot or monochrome OpenAI knot for Codex, beside the state), project
  (repo name, else cwd basename), short session ID, title, and a one-line current
  task. Details expand to the full message and context. Text is rendered literally.
  Above Details is a live timing line from `times` ("Running for 4m 12s",
  "Waiting for 2m 5s · task running for 10m 0s", "Finished 2 minutes ago · ran
  for 4m 30s"), refreshed every second; its tooltip lists the exact timestamps.
  A working row whose agent has been silent for a minute adds "last update …".
- **Filter and sort** (tray app only, view-only): a slim bar under the header
  with chips All · Needs you (alerting) · Asking (`input_needed*`) · Working ·
  Done (`done*`/`failed*`), each with a count, and a sort menu: Needs you first
  (default), Working first, Last update, or First seen, with ↓/↑ for newest or
  oldest first within that order. Opens on All every launch.
- **Row buttons:** **Focus** returns to the agent's terminal and changes nothing
  else (afterwards it only reports the time through `POST /focused/{id}`). **Dismiss** (alerting rows only) acknowledges that row; others keep
  alerting. **×** (Clear) removes the row.
- **Web interface** (`GET /`): the same tasks and sound controls in a browser,
  plus Clear all, Resume, the REST reference, curl examples, and a "last request"
  line showing the exact call each button made.
- **Self-documenting app:** each tray-app button's tooltip names its REST call,
  and the footer's **Web API ↗** opens the web interface.
- **Sound bar:** **Stop sound** acknowledges every alerting row. **Snooze 1/5 min**
  mutes all sound until a recorded wall-clock time. The service compares it with
  the clock whenever the app polls, and resumes the loop after it passes. A
  countdown shows while snoozed.
- **Sound:** one shared loop that escalates over time (`config/notify_config.yaml`
  gap, jitter, and escalation schedules). Questions and failures use the await
  sound; finished turns use the done sound.
- **Window and tray:** a frameless, always-on-top window on every Space and over
  full-screen apps. Only new alerting rows open it. Clearing the last row hides
  it. Hide, Escape, and closing send it to the tray, which recalls it. The tray
  dot and tooltip count tasks needing attention. The app reconnects after service
  restarts and keeps its last snapshot while offline.
- **Task backup:** the service writes rows, the snooze deadline, and waiting
  tools as TOML to `/tmp/desktop-notify/state-<port>.toml` (directory 0700,
  file 0600, atomic rename) every 5 minutes when changed and on SIGTERM/SIGINT,
  waiting at most 1 s at shutdown. On start it restores that file once, revalidating
  every row, then resumes sound for alerting rows. Missing, corrupt, other-format,
  or non-private files are ignored; write failures are logged. It survives
  service restarts, not reboots. SIGKILL cannot be caught, so it keeps the
  last periodic backup. Delete the file to start empty.
- **Logs:** `target/hook-events.jsonl` (rolling, one line per hook decision) and
  `target/desktop-notify.log` (service log with state transitions, rotated at
  5 MB). See troubleshooting.

### Focus: finding the exact terminal

Capture happens in the hook, at alert time:

0. **Shared Codex daemon:** its process environment belongs to the terminal that
   started the daemon, not necessarily this session. The terminal-side
   `scripts/codex_terminal.py` bridge uses the normal shared daemon through a
   private Unix WebSocket relay. Before forwarding a turn request or a successful
   start/resume/fork response, it binds that exact session ID to the live terminal
   client's PID, start time, and allowed terminal environment. Hooks load this
   binding from ignored `target/session-origins/`, validate the process lifetime,
   and discover origin from that client. Claude and embedded Codex still use
   their direct ancestry. An unbound detached Codex daemon sends an empty origin
   to clear stale hints; it must never reuse the daemon's terminal environment.
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
   `tab_id`, and `terminal_id`. A nonblocking per-app/TTY lock protects the whole
   probe from concurrent hooks on the same surface; contending hooks use existing
   hints, and failed scripting calls stop retries. The result is cached in ignored
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

`origin` fields: `terminal_app`, `app_pid`, `pid`, `terminal_id`, `tab_id`,
`window_id`, `window_title`, `tty`, `tmux_socket`, `tmux_pane`, `tmux_client`,
`tmux_server_pid`, `tmux_session`, `tmux_session_name`, `tmux_window`,
`tmux_window_index`, `tmux_window_name`. All are optional and validated in
`Origin::validate`.

## Checking and troubleshooting

- **Check a hook:** pipe a Stop event into the installed command, then inspect
  the row's `origin`:
  `echo '{"hook_event_name":"Stop","session_id":"test","cwd":"'$PWD'","last_assistant_message":"hi"}' | <python> scripts/codex_hook.py`,
  then `curl -s 127.0.0.1:43110/notifications`. Dismiss the test row afterwards
  with `POST /dismiss/<id>`.
- **Check capture alone:** run
  `cd scripts && python3 -c 'import notification_origin as o; print(o.capture_origin())'`
  from the terminal you want identified. Expect `terminal_id` for Ghostty and
  `tmux_*` fields inside tmux.
- **Why did a row get this state?** `target/hook-events.jsonl` has one JSON line
  per hook run: event, session, tool, chosen endpoint (`action`), for Stop the
  `closing` paragraph that was classified, the resulting `state`, any `error`,
  and `ms`. It trims itself to its newest half past 512 KB. The service log
  `target/desktop-notify.log` records each `session …: Old -> New` transition and
  rotates to `.1` past 5 MB when a hook starts the service.
- **No row appears:** make sure the hook is installed for this agent and the
  session was started after installing. Check `target/desktop-notify.log` and
  that `/health` answers.
- **Focus reaches Ghostty but not the window:** the row predates the fix, the
  probe was disabled, a fixed Ghostty `title` config ignores OSC 2, or macOS
  denied the process permission to control Ghostty. Allow it in System Settings →
  Privacy & Security → Automation, or run `scripts/register_terminal.py` in that
  window.
- **Codex focuses another session while Claude works:** check for a shared Codex
  daemon. Install `python3 scripts/install_terminal_bridge.py --install` and load
  its shell function (new shells source it via `~/.config/shell/aliases.sh`).
  Resume existing sessions through that function to register their terminal.
  The bridge keeps the shared daemon enabled; no `--no-daemon` workaround is
  needed. The hook log now includes the resulting origin for diagnosis.
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
`--restart` additionally restarts the service (rows return from its task backup).
`python3 scripts/test_codex_live.py` uses the configured Codex model/account.
Use these deliberately for relevant integration changes and report what ran.

## Repository hygiene

- Keep generated bundles, logs, terminal registrations, caches, and Tauri schemas
  in their existing ignored locations. Do not commit `target/` or local hook config.
- Keep rule content in `AGENTS.md`. Every rule file must have a sibling relative
  symlink `CLAUDE.md -> AGENTS.md`; do not maintain separate copies.
