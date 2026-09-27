use std::time::Instant;

use actix_web::{web, HttpResponse};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::audio_player::LoopSpec;
use crate::notifications::{DesktopStatus, Notification, NotificationState};
use crate::server_state::ServerState;

#[derive(Deserialize)]
pub struct NotificationRequest {
  title: String,
  message: String,
  session_id: Option<String>,
  context: Option<notify_types::SessionContext>,
  origin: Option<notify_types::Origin>,
}

pub async fn awaiting_user_input(
  state: web::Data<ServerState>,
  request: web::Json<NotificationRequest>,
) -> HttpResponse {
  notify(&state, request.into_inner(), false)
}

pub async fn all_tasks_finished(
  state: web::Data<ServerState>,
  request: web::Json<NotificationRequest>,
) -> HttpResponse {
  notify(&state, request.into_inner(), true)
}

fn notify(state: &ServerState, request: NotificationRequest, done: bool) -> HttpResponse {
  if let Some(context) = &request.context {
    if let Err(error) = context.validate() {
      return HttpResponse::BadRequest().body(error);
    }
  }
  if request
    .session_id
    .as_ref()
    .is_some_and(|id| id.trim().is_empty() || id.len() > 256 || id.chars().any(char::is_control))
  {
    return HttpResponse::BadRequest()
      .body("session_id must be nonblank, at most 256 bytes, and contain no control characters\n");
  }
  if let Some(origin) = &request.origin {
    if let Err(error) = origin.validate() {
      return HttpResponse::BadRequest().body(error);
    }
  }
  let title = request.title.trim();
  let message = request.message.trim();
  if title.is_empty()
    || message.is_empty()
    || title.chars().count() > 200
    || message.chars().count() > 4000
  {
    return HttpResponse::BadRequest()
      .body("title must be 1–200 characters; message must be 1–4000 characters\n");
  }
  let (primary, extras, kind) = if done {
    (
      &state.config.alert_done_sound,
      &state.config.extra_alert_done_sounds,
      "all_tasks_finished",
    )
  } else {
    (
      &state.config.alert_await_user_input_sound,
      &state.config.extra_alert_await_sounds,
      "awaiting_user_input",
    )
  };
  let Some(primary) = primary else {
    return HttpResponse::ServiceUnavailable().body("notification sound is not configured\n");
  };
  let pool: Vec<_> = std::iter::once(primary.clone())
    .chain(extras.iter().cloned())
    .collect();
  if pool.iter().any(|path| !path.is_file()) {
    return HttpResponse::ServiceUnavailable().body("configured notification sound is missing\n");
  }
  let mut notification = Notification {
    id: format!("{:032x}", rand::random::<u128>()),
    session_id: request.session_id,
    silenced: false,
    kind: kind.into(),
    title: title.into(),
    message: message.into(),
    context: Default::default(),
    origin: request.origin,
  };
  // Serialize replacement and dismissal with their corresponding audio command.
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  if let Some(index) = current
    .active
    .iter()
    .position(|n| n.session_id == notification.session_id)
  {
    let previous = current.active.remove(index);
    if notification.session_id.is_some() {
      notification.context = previous.context;
    }
    if notification.origin.is_none() {
      notification.origin = previous.origin;
    }
  }
  notification
    .context
    .apply(request.context.unwrap_or_default());
  current.active.insert(0, notification.clone());
  reconcile_audio(state, &mut current);
  HttpResponse::Ok().json(notification)
}

pub async fn current_notification(state: web::Data<ServerState>) -> HttpResponse {
  let current = lock_and_resume(&state);
  HttpResponse::Ok().json(current.active.first())
}

pub async fn list_notifications(state: web::Data<ServerState>) -> HttpResponse {
  let current = lock_and_resume(&state);
  HttpResponse::Ok().json(&current.active)
}

/// A snooze is a recorded wall-clock deadline, not a sleeping timer. A deadline
/// further out than the longest snooze means the clock moved backwards, so it is
/// treated as elapsed rather than muting sound indefinitely.
fn snooze_active(current: &NotificationState, now: DateTime<Utc>) -> bool {
  current
    .snoozed_until
    .is_some_and(|until| until > now && until - now <= notify_types::MAX_SNOOZE)
}

/// Locks state and resumes sound if a snooze deadline has passed. The tray app
/// polls `/notifications` every 400 ms, so that poll is what notices expiry;
/// playback otherwise remains unchanged by reads.
pub(crate) fn lock_and_resume(state: &ServerState) -> std::sync::MutexGuard<'_, NotificationState> {
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  if current.snoozed_until.is_some() && !snooze_active(&current, Utc::now()) {
    reconcile_audio(state, &mut current);
  }
  current
}

pub(crate) fn sound_state(current: &NotificationState) -> notify_types::SoundState {
  notify_types::SoundState {
    snoozed_until: current.snoozed_until,
    alerting: current.active.iter().any(|n| !n.silenced),
  }
}

/// One shared loop: questions take priority, then the most recent completion.
/// Removing/silencing another row must not restart or stop this loop. A global
/// snooze mutes the loop without silencing rows, so it resumes afterwards.
fn reconcile_audio(state: &ServerState, current: &mut NotificationState) {
  if current.snoozed_until.is_some() && !snooze_active(current, Utc::now()) {
    current.snoozed_until = None;
  }
  let next = if current.snoozed_until.is_some() {
    None
  } else {
    current
      .active
      .iter()
      .find(|n| !n.silenced && n.kind == "awaiting_user_input")
      .or_else(|| current.active.iter().find(|n| !n.silenced))
  };
  let next_id = next.map(|n| n.id.clone());
  if current.audio_id == next_id {
    return;
  }
  if let Some(notification) = next {
    let (primary, extras, name) = if notification.kind == "awaiting_user_input" {
      (
        &state.config.alert_await_user_input_sound,
        &state.config.extra_alert_await_sounds,
        "await",
      )
    } else {
      (
        &state.config.alert_done_sound,
        &state.config.extra_alert_done_sounds,
        "done",
      )
    };
    state.audio.play_loop(LoopSpec {
      name: name.into(),
      pool: primary
        .iter()
        .cloned()
        .chain(extras.iter().cloned())
        .collect(),
      gap_millis_schedule: state.config.loop_gap_schedule_millis(),
      jitter_millis_schedule: state.config.loop_jitter_schedule_millis(),
      escalate_waits_secs: state.config.escalate_waits_secs(),
    });
  } else {
    state.audio.stop_all();
  }
  current.audio_id = next_id;
}

#[derive(Serialize)]
struct DismissResponse {
  stopped: bool,
}

pub async fn dismiss(state: web::Data<ServerState>, id: web::Path<String>) -> HttpResponse {
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  let stopped = current.active.iter().any(|n| n.id == *id);
  if stopped {
    current.active.retain(|n| n.id != *id);
    reconcile_audio(&state, &mut current);
  }
  HttpResponse::Ok().json(DismissResponse { stopped })
}

pub async fn silence(state: web::Data<ServerState>, id: web::Path<String>) -> HttpResponse {
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  let stopped = if let Some(notification) = current
    .active
    .iter_mut()
    .find(|n| n.id == *id && !n.silenced)
  {
    notification.silenced = true;
    true
  } else {
    false
  };
  if stopped {
    reconcile_audio(&state, &mut current);
  }
  HttpResponse::Ok().json(DismissResponse { stopped })
}

pub async fn sound(state: web::Data<ServerState>) -> HttpResponse {
  let current = lock_and_resume(&state);
  HttpResponse::Ok().json(sound_state(&current))
}

/// Global Stop sound: silences every current row and cancels any snooze. Later
/// updates are fresh alerts and sound again.
pub async fn silence_all(state: web::Data<ServerState>) -> HttpResponse {
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  for notification in &mut current.active {
    notification.silenced = true;
  }
  current.snoozed_until = None;
  reconcile_audio(&state, &mut current);
  HttpResponse::Ok().json(sound_state(&current))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnoozeRequest {
  /// Snooze length in whole seconds, converted to a `TimeDelta` on receipt.
  seconds: i64,
}

/// Mutes the shared loop until now + `seconds`. Rows keep their silenced flags,
/// and alerts arriving during the snooze wait for it to end. Snoozing again
/// replaces the previous deadline.
pub async fn snooze(
  state: web::Data<ServerState>,
  request: web::Json<SnoozeRequest>,
) -> HttpResponse {
  let duration = TimeDelta::try_seconds(request.seconds)
    .filter(|duration| *duration > TimeDelta::zero() && *duration <= notify_types::MAX_SNOOZE);
  let Some(duration) = duration else {
    return HttpResponse::BadRequest().body(format!(
      "seconds must be between 1 and {}\n",
      notify_types::MAX_SNOOZE.num_seconds()
    ));
  };
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  current.snoozed_until = Some(Utc::now() + duration);
  reconcile_audio(&state, &mut current);
  HttpResponse::Ok().json(sound_state(&current))
}

pub async fn resume_sound(state: web::Data<ServerState>) -> HttpResponse {
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  current.snoozed_until = None;
  reconcile_audio(&state, &mut current);
  HttpResponse::Ok().json(sound_state(&current))
}

pub async fn desktop_status(
  state: web::Data<ServerState>,
  status: web::Json<DesktopStatus>,
) -> HttpResponse {
  let mut current = lock_and_resume(&state);
  current.desktop = status.into_inner();
  current.desktop_seen = Some(Instant::now());
  HttpResponse::Ok().finish()
}

#[derive(Serialize)]
struct Health {
  service: &'static str,
  api_version: u32,
  pid: u32,
}

pub async fn health() -> HttpResponse {
  HttpResponse::Ok().json(Health {
    service: "desktop-notify",
    api_version: 3,
    pid: std::process::id(),
  })
}
