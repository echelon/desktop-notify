# agent-notify-server

Local REST server with looping sounds and actionable desktop alerts for agents.
See the [workspace README](../../README.md) for the Tauri tray app and the
installed Codex/Claude Code hooks. Replaces `afplay` bash loops that were
prone to leaving zombie processes — the server owns the audio pipeline and
exits cleanly with Ctrl+C.

## Run

```sh
cargo run --bin agent-notify-server
# or, for a release-mode binary:
cargo run --release --bin agent-notify-server
```

The server listens on `127.0.0.1:43110` by default. Override with
`HTTP_BIND_ADDRESS`. Override the config path with `NOTIFY_CONFIG_PATH`
(defaults to this crate's `config/notify_config.yaml`, resolved at build time).

## Endpoints

Open **`http://127.0.0.1:43110/`** in a browser for the web interface: live tasks
with Dismiss/Clear, sound controls, and the API reference. `GET /` is permanent.
The full contract (task states, request fields, and which client calls each
endpoint) is in the workspace [AGENTS.md](../../AGENTS.md#http-api-current-api-version-4).

| Method | Path | Behavior |
| --- | --- | --- |
| GET | `/` | Web interface and API reference (permanent). |
| POST | `/awaiting_user_input` | `{title, message, session_id?, context?, origin?, agent?, tool_use_id?, turn_started_at?}`; row becomes `input_needed`. |
| POST | `/all_tasks_finished` | Same body without `tool_use_id`; row becomes `done`. |
| POST | `/task_failed` | Same; row becomes `failed`. |
| POST | `/working` | `{session_id, title?, message?, context?, origin?, agent?, only_if_waiting?, tool_use_id?}`; row becomes `working` (no alert). Returns `{updated, notification?}`. |
| POST | `/acknowledge/{id}` | Dismiss one alerting row; it stays listed in its quiet state. |
| POST | `/dismiss/{id}` | Clear one row. |
| POST | `/focused/{id}` | Record that the app focused this row's terminal; changes nothing else. |
| GET | `/notifications` | All rows, newest first. |
| GET | `/sound` | `{snoozed_until, alerting}`. |
| POST, GET | `/sound/stop` | Acknowledge every alerting row and cancel any snooze; rows stay. |
| POST | `/sound/snooze` | `{seconds}` (1–86400): mute until that wall-clock time. |
| POST | `/sound/resume` | End a snooze early. |
| GET | `/stop` | Legacy `stop-sound` alias: same as `GET /sound/stop`; rows stay. |
| POST | `/stop` | Clear every row, cancel any snooze, stop all audio. |
| GET | `/state` | Rows, sound, audio engine, config, and desktop app status. Read-only. |
| GET | `/health` | `{service, api_version, pid}`. |
| POST | `/desktop/status` | Tray app heartbeat. |

Rows carry `state` (`working`, `input_needed`, `input_needed_ignored`, `done`,
`done_acknowledged`, `failed`, `failed_acknowledged`) and optionally `agent`
(`claude_code`, `codex`, or `unknown`). Updates replace only the matching
`session_id`; omitted IDs share one unassigned row, and named sessions keep their
context, origin, and agent when an update omits them. One shared loop plays for
the newest `input_needed`, else `failed`, else `done` row. A snooze stores its
deadline and mutes the loop without changing rows; the next request after it
passes (the app polls every 400 ms) resumes sound. Rows are held in memory.

Rows also carry optional service-assigned `times` (RFC 3339 UTC): `tracked_since`
(first report, kept until cleared), `updated_at` (latest agent report, including
an `only_if_waiting` call that changed nothing), `task_started_at` (the prompt
that started the current task; an alert's optional `turn_started_at` fills it
only when the service did not see the start and it falls after the previous task ended), `task_finished_at` (`done`/`failed` only),
`waiting_since` (`input_needed*` only), `dismissed_at` (cleared by the next
agent update), `last_request_at` (any request touching the row),
`user_action_at` (Dismiss, Stop sound, or Focus from the app or web), and
`user_input_at` (a submitted prompt or answered question, from the hooks).
Moments the service did not see are omitted.

The sound-only `/alert_*` and `/loop_*` endpoints, `GET /notification`,
`POST /silence/{id}`, and the `kind`/`silenced` row fields were removed in favor
of the endpoints above.

### `/state` response shape

```json
{
  "notifications": [],
  "audio_notification_id": null,
  "sound": {"snoozed_until": null, "alerting": false},
  "desktop": {"presentation": "tauri", "window_visible": false, "pid": 123, "displayed_ids": []},
  "desktop_connected": true,
  "audio": {
    "loop_playing": true,
    "loop_name": "done",
    "voices_active": 2,
    "current_stage": 1,
    "current_gap_millis": 1000,
    "current_jitter_millis": 750,
    "loop_pool_size": 4,
    "loop_uptime_secs": 17
  },
  "config": {
    "alert_done_sound": "…/smrpg_flower.wav",
    "alert_await_user_input_sound": "…/smrpg_ghost.wav",
    "extra_alert_done_count": 3,
    "extra_alert_await_count": 3,
    "gap_schedule_millis": [2000, 1000, 500, 200],
    "jitter_schedule_millis": [1500, 750, 400, 180],
    "escalate_waits_secs": [10, 20, 30]
  }
}
```

`loop_name` is `done` or `await`. When idle, `loop_playing` is `false` and the
loop fields collapse to zero/null.

## Config

`config/notify_config.yaml`:

```yaml
alert_done_sound: sounds/smrpg_flower.wav
alert_await_user_input_sound: sounds/smrpg_ghost.wav

extra_alert_done_sounds:
  - sounds/smrpg_specialflower.wav
  - sounds/smrpg_correct.wav
extra_alert_await_sounds:
  - sounds/smrpg_wrong.wav
  - sounds/smrpg_drybones_crumble.wav

loop_alert_timeout_millis: 2000
loop_alert_timeout_millis_1: 1000
loop_alert_timeout_millis_2: 500
loop_alert_timeout_millis_3: 200
loop_alert_jitter_millis: 250
loop_alert_jitter_millis_1: 150
loop_alert_jitter_millis_2: 80
loop_alert_jitter_millis_3: 40
escalate_wait_1: 15
escalate_wait_2: 30
escalate_wait_3: 45
```

- Paths can be absolute, or relative to the YAML file's directory.
- WAV and MP3 are both supported (decoded via rodio + symphonia).
- `loop_alert_timeout_millis` is the gap *between* consecutive plays of a
  single voice at stage 0. Omit to replay back-to-back.
- `loop_alert_timeout_millis_{1,2,3}` override the gap at each escalation
  stage. Each falls back to the previous stage when unset — so you can set
  just `_1: 200` to drop straight to a 200ms gap from voice 2 onward.
- `loop_alert_jitter_millis[_1|_2|_3]` add `+/- rand(0..=jitter)` to each
  sleep (clamped at 0). Same fallback chain as the timeouts. Useful to
  keep voices from re-aligning even when they share a gap. The *upcoming*
  stage's jitter is also applied to the escalation wait itself, so voices
  2/3/4 don't always land exactly on `escalate_wait_N` — their phase
  drifts by `+/- rand(0..=jitter)` ms.
- **Escalation**: an alert's loop starts one voice immediately. At
  `escalate_wait_1` / `escalate_wait_2` / `escalate_wait_3` seconds, a
  second / third / fourth concurrent voice joins the mix. New voices are
  taken from `extra_alert_<state>_sounds` in order; when that pool is
  exhausted the supervisor cycles back through the full pool
  (primary + extras), so existing voices double up. Each voice runs in
  its own thread and drifts naturally relative to the others.
- All voices in a session share the *current* stage's gap, so existing
  voices also speed up when the supervisor advances stages.

## Shutdown

Ctrl+C exits cleanly in ~0.3s, even mid-playback. The server has
`shutdown_timeout(0)` and signals the audio engine to drop any in-flight
sound the moment SIGINT arrives — no zombie afplay processes.
