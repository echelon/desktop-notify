# Notification frontend

Inherit the root rules. This is plain HTML, CSS, and JavaScript bundled directly
by Tauri's `frontendDist`; there is no package manifest, framework, or Node build.
Use two-space JavaScript indentation and follow the existing CSS style/tokens.

## Data and rendering

- Receive state through Tauri's `notification-state` event and `get_snapshot`;
  invoke native commands for actions. Rust owns HTTP and OS integration.
- Subscribe before loading the initial snapshot, then signal `window_ready`.
- Treat all notification/context/error text as literal text via `textContent`.
  Do not render conversation text as HTML, Markdown, or executable content.
- Keep DOM rows keyed by notification ID and reuse unchanged nodes. Polling or
  other rows' updates must preserve expanded details, selection, keyboard focus,
  and per-row status. Do not rebuild the list with `innerHTML` on every update.
- Scope pending actions, focus errors, and statuses to the row ID. A delayed
  result for a removed/replaced ID must never update its successor.
- Preserve snapshots during disconnection. Disable service-dependent row actions
  while offline; locally available Focus can remain usable.

## Product behavior

- Keep Focus, Dismiss, Stop sound, Clear, and Hide distinct. Focus must not invoke
  hide, acknowledge, or clear. (The native command reports `POST /focused/{id}`
  afterwards, which only records a timestamp.) Dismiss (`acknowledge_notification`) appears only on
  alerting rows and acknowledges that row alone. Stop sound and Snooze are global
  controls in the bar above the footer; they retain every row. Clear (×) removes
  only its row. Hide and keyboard shortcuts only hide the window.
- Filter chips (All, Needs you, Asking, Working, Done, with counts) and the sort
  menu (Needs you first, Working first, Last update, First seen, plus a
  newest/oldest toggle) are view-only: never persist them, call the service, or
  change the tray count. They reset to All and Needs you first, newest first on
  launch. Filtered-out rows keep their nodes so details and statuses survive.
- Label rows by `state`; quiet rows are dimmed and `working` shows a pulse, which
  honors reduced motion.
- Stay self-documenting: each action's tooltip names its REST call, and the
  footer's API link (`open_web_interface`) opens the service's web interface.
- The service owns the snooze deadline. The UI only compares it with `Date.now()`
  for the countdown; never resume or re-snooze from a UI timer.
- Disable Focus when usable origin hints are absent. Reflect pending operations
  on only the affected row and surface failures there.
- Keep the compact hierarchy: project label, alert title, one ellipsized task
  line, then expandable full message/context. Prefer repo name over cwd basename
  for the project and current ask over work arc for the preview.
- Each row shows a timing line above its details, built from the service's `times`; the
  one-second interval rewrites it only when the text changes. Omit parts whose
  timestamps are absent, and hide the line entirely for rows without times.
- Hide absent context without placeholder noise; title/message-only alerts still
  work. Avoid repeating a known directory prefix in both title and project label.
  Make full text and paths available in details/tooltips.
- Preserve the dark frameless tray-app layout, scrolling at small window sizes,
  draggable header, resize handle, text selection, accessible labels, live status
  messages, and visible keyboard focus. Keep `[hidden]` reliably hidden.

## Verification

Run `node --test scripts/test_ui.mjs` from the root for rendering/action changes.
The tests use a small DOM/Tauri mock with Node's built-in test runner. Cover row
identity, delayed actions, optional context, literal markup, and offline behavior
when relevant. For layout changes, rebuild with `python3 scripts/build.py` and
inspect the real Tauri window with long text, several sessions, and narrow sizes;
the mock cannot verify CSS or native window behavior.
