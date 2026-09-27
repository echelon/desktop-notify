use std::path::PathBuf;
use std::time::Instant;

use actix_web::{web, HttpResponse};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::audio_player::LoopSpec;
use crate::notifications::{DesktopStatus, Notification, NotificationState};
use crate::server_state::ServerState;
use notify_types::TaskState;

#[derive(Deserialize)]
pub struct NotificationRequest {
  title: String,
  message: String,
  session_id: Option<String>,
  context: Option<notify_types::SessionContext>,
  origin: Option<notify_types::Origin>,
  /// The tool call a question/permission request is waiting on, when known.
  tool_use_id: Option<String>,
  agent: Option<notify_types::Agent>,
}

pub async fn awaiting_user_input(
  state: web::Data<ServerState>,
  request: web::Json<NotificationRequest>,
) -> HttpResponse {
  notify(&state, request.into_inner(), TaskState::InputNeeded)
}

pub async fn all_tasks_finished(
  state: web::Data<ServerState>,
  request: web::Json<NotificationRequest>,
) -> HttpResponse {
  notify(&state, request.into_inner(), TaskState::Done)
}

pub async fn task_failed(
  state: web::Data<ServerState>,
  request: web::Json<NotificationRequest>,
) -> HttpResponse {
  notify(&state, request.into_inner(), TaskState::Failed)
}

/// The loop an alerting state plays: waiting and failures need the user, so
/// they share the await sound; completions use the done sound.
fn sound_pool(state: &ServerState, task: TaskState) -> Option<(&'static str, Vec<PathBuf>)> {
  let (name, primary, extras) = match task {
    TaskState::InputNeeded | TaskState::Failed => (
      "await",
      &state.config.alert_await_user_input_sound,
      &state.config.extra_alert_await_sounds,
    ),
    TaskState::Done => (
      "done",
      &state.config.alert_done_sound,
      &state.config.extra_alert_done_sounds,
    ),
    _ => return None,
  };
  let primary = primary.as_ref()?;
  Some((
    name,
    std::iter::once(primary.clone())
      .chain(extras.iter().cloned())
      .collect(),
  ))
}

fn validate_metadata(
  session_id: Option<&String>,
  context: Option<&notify_types::SessionContext>,
  origin: Option<&notify_types::Origin>,
) -> Result<(), HttpResponse> {
  if let Some(error) = context.and_then(|c| c.validate().err()) {
    return Err(HttpResponse::BadRequest().body(error));
  }
  if session_id
    .is_some_and(|id| id.trim().is_empty() || id.len() > 256 || id.chars().any(char::is_control))
  {
    return Err(HttpResponse::BadRequest().body(
      "session_id must be nonblank, at most 256 bytes, and contain no control characters\n",
    ));
  }
  if let Some(error) = origin.and_then(|o| o.validate().err()) {
    return Err(HttpResponse::BadRequest().body(error));
  }
  Ok(())
}

fn valid_tool_use_id(id: Option<&String>) -> Result<(), HttpResponse> {
  if id.is_some_and(|id| id.trim().is_empty() || id.len() > 256 || id.chars().any(char::is_control))
  {
    return Err(HttpResponse::BadRequest().body(
      "tool_use_id must be nonblank, at most 256 bytes, and contain no control characters\n",
    ));
  }
  Ok(())
}

fn valid_text(title: &str, message: &str) -> Result<(), HttpResponse> {
  if title.is_empty()
    || message.is_empty()
    || title.chars().count() > 200
    || message.chars().count() > 4000
  {
    return Err(
      HttpResponse::BadRequest()
        .body("title must be 1–200 characters; message must be 1–4000 characters\n"),
    );
  }
  Ok(())
}

fn fresh_id() -> String {
  format!("{:032x}", rand::random::<u128>())
}

/// Replaces the session's row with `notification` at the front. Named sessions
/// keep their context, origin, and agent unless the update supplies them. Any tool the
/// previous row waited on is forgotten; `waiting_on` records the new one.
fn replace_row(
  current: &mut NotificationState,
  mut notification: Notification,
  context: Option<notify_types::SessionContext>,
  waiting_on: Option<String>,
) -> Notification {
  if let Some(session) = &notification.session_id {
    match waiting_on {
      Some(tool) => current.waiting_tools.insert(session.clone(), tool),
      None => current.waiting_tools.remove(session),
    };
  }
  if let Some(index) = current
    .active
    .iter()
    .position(|n| n.session_id == notification.session_id)
  {
    let previous = current.active.remove(index);
    log::info!(
      "session {:?}: {:?} -> {:?}",
      notification.session_id,
      previous.state,
      notification.state
    );
    if notification.session_id.is_some() {
      notification.context = previous.context;
    }
    if notification.origin.is_none() {
      notification.origin = previous.origin;
    }
    if notification.agent.is_none() {
      notification.agent = previous.agent;
    }
  }
  notification.context.apply(context.unwrap_or_default());
  current.active.insert(0, notification.clone());
  notification
}

fn notify(state: &ServerState, request: NotificationRequest, task: TaskState) -> HttpResponse {
  if let Err(response) = validate_metadata(
    request.session_id.as_ref(),
    request.context.as_ref(),
    request.origin.as_ref(),
  ) {
    return response;
  }
  let (title, message) = (request.title.trim(), request.message.trim());
  if let Err(response) =
    valid_text(title, message).and(valid_tool_use_id(request.tool_use_id.as_ref()))
  {
    return response;
  }
  let Some((_, pool)) = sound_pool(state, task) else {
    return HttpResponse::ServiceUnavailable().body("notification sound is not configured\n");
  };
  if pool.iter().any(|path| !path.is_file()) {
    return HttpResponse::ServiceUnavailable().body("configured notification sound is missing\n");
  }
  let notification = Notification {
    id: fresh_id(),
    session_id: request.session_id,
    state: task,
    agent: request.agent,
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
  let waiting_on = request
    .tool_use_id
    .filter(|_| task == TaskState::InputNeeded);
  let notification = replace_row(&mut current, notification, request.context, waiting_on);
  reconcile_audio(state, &mut current);
  HttpResponse::Ok().json(notification)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingRequest {
  session_id: String,
  title: Option<String>,
  message: Option<String>,
  context: Option<notify_types::SessionContext>,
  origin: Option<notify_types::Origin>,
  /// Only resume a waiting row (after the user answered or approved); never
  /// create a row or replace a finished one. Used by per-tool hooks.
  #[serde(default)]
  only_if_waiting: bool,
  /// The tool call that just finished. A row waiting on a different call stays
  /// waiting; without IDs on both sides, any finished tool resumes it.
  tool_use_id: Option<String>,
  agent: Option<notify_types::Agent>,
}

#[derive(Serialize)]
struct WorkingResponse {
  updated: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  notification: Option<Notification>,
}

/// Marks a session busy: a prompt was submitted or the agent resumed after
/// input. Working rows never alert, so this also stops that row's sound.
pub async fn working(
  state: web::Data<ServerState>,
  request: web::Json<WorkingRequest>,
) -> HttpResponse {
  let request = request.into_inner();
  if let Err(response) = validate_metadata(
    Some(&request.session_id),
    request.context.as_ref(),
    request.origin.as_ref(),
  ) {
    return response;
  }
  if let Err(response) = valid_tool_use_id(request.tool_use_id.as_ref()) {
    return response;
  }
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  let other_tool = match (
    current.waiting_tools.get(&request.session_id),
    &request.tool_use_id,
  ) {
    (Some(waiting), Some(finished)) => waiting != finished,
    _ => false,
  };
  let previous = current
    .active
    .iter()
    .find(|n| n.session_id.as_deref() == Some(request.session_id.as_str()));
  if request.only_if_waiting && (other_tool || !previous.is_some_and(|n| n.state.is_waiting())) {
    return HttpResponse::Ok().json(WorkingResponse {
      updated: false,
      notification: None,
    });
  }
  // A resumed question keeps its text until the agent reports something new.
  let title = request
    .title
    .as_deref()
    .map(str::trim)
    .or(previous.map(|n| n.title.as_str()))
    .unwrap_or("Working")
    .to_owned();
  let message = request
    .message
    .as_deref()
    .map(str::trim)
    .or(previous.map(|n| n.message.as_str()))
    .unwrap_or("The agent is working.")
    .to_owned();
  if let Err(response) = valid_text(&title, &message) {
    return response;
  }
  let notification = Notification {
    id: fresh_id(),
    session_id: Some(request.session_id),
    state: TaskState::Working,
    agent: request.agent,
    title,
    message,
    context: Default::default(),
    origin: request.origin,
  };
  let notification = replace_row(&mut current, notification, request.context, None);
  reconcile_audio(&state, &mut current);
  HttpResponse::Ok().json(WorkingResponse {
    updated: true,
    notification: Some(notification),
  })
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
    alerting: current.active.iter().any(|n| n.state.is_alerting()),
  }
}

/// One shared loop for the newest alerting row: questions first, then
/// failures, then completions. Changes to other rows must not restart or stop
/// this loop. A global snooze mutes the loop without changing row states, so
/// it resumes afterwards.
fn reconcile_audio(state: &ServerState, current: &mut NotificationState) {
  if current.snoozed_until.is_some() && !snooze_active(current, Utc::now()) {
    current.snoozed_until = None;
  }
  let next = if current.snoozed_until.is_some() {
    None
  } else {
    [TaskState::InputNeeded, TaskState::Failed, TaskState::Done]
      .into_iter()
      .find_map(|task| current.active.iter().find(|n| n.state == task))
  };
  let next_id = next.map(|n| n.id.clone());
  if current.audio_id == next_id {
    return;
  }
  match next.and_then(|n| sound_pool(state, n.state)) {
    Some((name, pool)) => state.audio.play_loop(LoopSpec {
      name: name.into(),
      pool,
      gap_millis_schedule: state.config.loop_gap_schedule_millis(),
      jitter_millis_schedule: state.config.loop_jitter_schedule_millis(),
      escalate_waits_secs: state.config.escalate_waits_secs(),
    }),
    None => state.audio.stop_all(),
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

/// Per-row dismissal: an alerting row moves to its quiet counterpart (done →
/// done_acknowledged, input_needed → input_needed_ignored, failed →
/// failed_acknowledged) and stays listed. Other rows keep alerting. Stale or
/// already-quiet IDs are harmless.
pub async fn acknowledge(state: web::Data<ServerState>, id: web::Path<String>) -> HttpResponse {
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  let quiet = current
    .active
    .iter_mut()
    .find(|n| n.id == *id)
    .and_then(|n| n.state.acknowledged().map(|quiet| (n, quiet)));
  let stopped = quiet.is_some();
  if let Some((notification, quiet)) = quiet {
    log::info!(
      "session {:?}: {:?} -> {quiet:?} (dismissed)",
      notification.session_id,
      notification.state
    );
    notification.state = quiet;
    reconcile_audio(&state, &mut current);
  }
  HttpResponse::Ok().json(DismissResponse { stopped })
}

pub async fn sound(state: web::Data<ServerState>) -> HttpResponse {
  let current = lock_and_resume(&state);
  HttpResponse::Ok().json(sound_state(&current))
}

/// Global Stop sound (`POST /sound/stop`): acknowledges every alerting row and
/// cancels any snooze. Rows stay listed; later updates are fresh alerts.
pub async fn stop_sound(state: web::Data<ServerState>) -> HttpResponse {
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  for notification in &mut current.active {
    if let Some(quiet) = notification.state.acknowledged() {
      notification.state = quiet;
    }
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

/// Mutes the shared loop until now + `seconds`. Rows keep their states,
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
    api_version: 4,
    pid: std::process::id(),
  })
}
