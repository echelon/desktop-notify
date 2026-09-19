use std::time::Instant;

use actix_web::{web, HttpResponse};
use serde::{Deserialize, Serialize};

use crate::audio_player::LoopSpec;
use crate::notifications::{DesktopStatus, Notification};
use crate::server_state::ServerState;

#[derive(Deserialize)]
pub struct NotificationRequest {
  title: String,
  message: String,
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
  let (primary, extras, name, kind) = if done {
    (
      &state.config.alert_done_sound,
      &state.config.extra_alert_done_sounds,
      "done",
      "all_tasks_finished",
    )
  } else {
    (
      &state.config.alert_await_user_input_sound,
      &state.config.extra_alert_await_sounds,
      "await",
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
  let notification = Notification {
    id: format!("{:032x}", rand::random::<u128>()),
    kind: kind.into(),
    title: title.into(),
    message: message.into(),
  };
  // Serialize replacement and dismissal with their corresponding audio command.
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  state.audio.play_loop(LoopSpec {
    name: name.into(),
    pool,
    gap_millis_schedule: state.config.loop_gap_schedule_millis(),
    jitter_millis_schedule: state.config.loop_jitter_schedule_millis(),
    escalate_waits_secs: state.config.escalate_waits_secs(),
  });
  current.active = Some(notification.clone());
  HttpResponse::Ok().json(notification)
}

pub async fn current_notification(state: web::Data<ServerState>) -> HttpResponse {
  let current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  HttpResponse::Ok().json(&current.active)
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
  let stopped = current.active.as_ref().is_some_and(|n| n.id == *id);
  if stopped {
    current.active = None;
    state.audio.stop_all();
  }
  HttpResponse::Ok().json(DismissResponse { stopped })
}

pub async fn desktop_status(
  state: web::Data<ServerState>,
  status: web::Json<DesktopStatus>,
) -> HttpResponse {
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
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
    api_version: 2,
    pid: std::process::id(),
  })
}
