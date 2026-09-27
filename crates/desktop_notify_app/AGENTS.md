# Tauri desktop shell

Inherit the root rules, including **two-space Rust indentation**. This crate owns
the native tray, window lifecycle, service connection, and terminal focus. The
frontend lives in `../../ui/` and has its own rules.

## Architecture and libraries

- Use Tauri 2 for the shell, reqwest for local HTTP, Tokio/Tauri async tasks for
  polling, and `notify-types` for notifications. Respect the exact Tauri and
  tauri-build pins in `Cargo.toml`.
- Keep native macOS work in the existing objc2/AppKit integration and fixed
  `src/focus/*.applescript` adapters, guarded by macOS cfgs.
- Rust owns HTTP requests. JavaScript uses Tauri commands and the
  `notification-state` event. Preserve the narrow capabilities and CSP in the
  Tauri configuration when adding a command or UI feature.
- `model.rs` holds pure visibility decisions that can be tested without a GUI.
  Keep blocking focus helpers on `spawn_blocking`, outside mutex guards.

## Window and connection behavior

- New/replaced notification IDs show the window. Unchanged, reordered, or silenced
  rows preserve visibility; removing one row must not hide other rows. Removing
  the final row hides the window. A user-hidden alert stays hidden until a new
  ID arrives or the tray is recalled.
- Preserve the `window_ready` handshake so startup alerts appear after the UI
  has subscribed and rendered. Do not show an uninitialized webview.
- Closing/minimizing and Escape hide to tray. Only Quit exits. Focus leaves the
  notification window available for later Stop sound/Clear actions.
- Preserve the macOS accessory app, always-on-top/all-Spaces behavior, full-screen
  auxiliary behavior, tray recall, and active-row indicator.
- Poll `/notifications` (currently every 400 ms), retain the last snapshot on
  connection failure, and reconnect without exiting or discarding rows. Report
  `/desktop/status` heartbeats, visibility, PID, and displayed IDs.
- Honor `--server-url`. Validate IDs before inserting them into action URLs and
  let polling reconcile successful silence/dismiss requests with current state.

## Focus lessons

- Focus is initiated by the row's button, never automatically on alert receipt.
  Read origin from the current snapshot by ID so stale requests cannot target a
  newer row. Validate it again before using native/process APIs.
- Try terminal session, TTY, window ID, exact window title, then application.
  Fall back when a narrower target has closed; report fallback warnings honestly.
  Only activate running apps and never guess an ambiguous application fallback.
- Ghostty's core surface ID is not its AppleScript terminal ID. Exact Ghostty
  window pairing comes from `scripts/register_terminal.py`; preserve that path.
- tmux selection requires the supplied absolute socket and a resolved stable pane
  ID. When `tmux_server_pid` is supplied, refuse a socket now served by another
  process: pane IDs restart with the server. Never use an arbitrary inherited
  server. A tmux failure still allows app focus with a warning.
- Pass origin strings as separate arguments to fixed scripts/commands; never
  interpolate them into shell or AppleScript source. Bound external helpers and
  kill/reap timed-out children, including stalled Automation permission prompts.
- Application activation needs no scripting permission; terminal scripting may
  need Automation and generic window-title matching may need Accessibility.
  Keep failure messages actionable and preserve the app's usage description.

## Verification and packaging

Run `cargo test --locked -p desktop-notify-app` for model/focus changes. Update
pure fallback and visibility tests where relevant, then use a real macOS check
for Spaces, full-screen windows, tray behavior, and actual focus targets.

Use `python3 scripts/build.py` from the root for the app used by hooks. It manually
assembles/signs the bundle; packaging changes may need coordinated updates to
that script, `tauri.conf.json`, and `Info.plist`. Keep assets in `icons-src/` and
`icons/` consistent; do not commit generated bundles or `gen/` schemas.
