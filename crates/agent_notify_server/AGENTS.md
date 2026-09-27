# Notification service

Inherit the root rules, including **two-space Rust indentation**.

## Structure

- `main.rs` wires Actix routes, shared state, app launch, and shutdown. The server
  uses one Actix worker. Keep handlers under `src/endpoints/`.
- `notifications.rs` owns the in-memory notification/desktop status model;
  `server_state.rs` shares it through `Arc<Mutex<_>>`.
- `audio_player.rs` owns rodio's output stream on a dedicated OS thread (the
  stream is not Send on every platform). Handlers send `AudioCommand`s through
  `AudioPlayerHandle`; do not move audio playback into request handlers.
- `config.rs` reads Serde YAML. Sound paths resolve relative to the YAML file,
  not the working directory. The default path is embedded at build time;
  `NOTIFY_CONFIG_PATH` supplies an override.

## State and API invariants

- Validate the entire request and required sound files before changing rows or
  audio. Invalid origin/context/session data must leave the previous state intact.
- Keep one row per `session_id`, with `None` as a separate legacy slot. An update
  replaces only its own row, gets a fresh 32-character hex ID, becomes unsilenced,
  and moves to the front. Do not key sessions by cwd, project name, or process ID.
- For named sessions, retain origin when omitted and merge context through
  `SessionContext::apply`. Unassigned requests do not inherit previous context.
- Serialize row mutation and audio reconciliation under the same notification
  lock. Enqueue audio commands before releasing it so concurrent actions cannot
  leave an older sound playing over newer state. Do not hold locks across awaits.
- Silence/dismiss operate on the current notification ID. Stale IDs are harmless
  and return `stopped: false`; never resolve them to the session's replacement.
- Audio selection is the newest unsilenced question, otherwise the newest
  unsilenced completion. If `audio_id` is unchanged, leave playback alone.
- `/notifications` includes silenced rows; `/notification` returns the newest row
  for older clients. Preserve legacy `/state` fields alongside the list and
  `audio_notification_id`. Read endpoints must not alter playback.
- `/stop` clears every row and all audio. Legacy sound-only loop endpoints clear
  the row list and replace the loop; one-shots mix over the loop.
- Keep `/health` service identity/version compatible with `scripts/codex_hook.py`.
  Coordinate any API version change with its health check.

## Audio and lifecycle

- Preserve one output stream, the reusable one-shot sink, and a cancellable loop
  supervisor with independent voices. Do not reintroduce shell/afplay loops.
- Loop replacement and shutdown must stop and join the old voices. Keep sleeps
  interruptible so Ctrl+C and Stop do not wait for a full sound or escalation gap.
- Keep gap/jitter fallback schedules and elapsed-time escalation semantics in
  `config.rs` consistent with the audio engine and API reference.
- Honor `NOTIFY_DISABLE_DESKTOP` for headless use. Desktop launch failures should
  be logged without taking down the service.

## Verification

Run `cargo test --locked -p agent-notify-server` and the root Rust format/check
commands for service changes. Extend `src/notification_tests.rs` using
`AudioPlayerHandle::recording()` and Actix's test service: assert both response
state and emitted audio commands without requiring speakers. Cover invalid
requests, stale IDs, replacement, independent sessions, and audio priority when
touching their behavior. Update both READMEs and `static/index.html` for API changes.
