use actix_web::{web, HttpResponse, Responder};

use crate::server_state::ServerState;

/// `POST /stop` (also `GET`): clears every row, any snooze, and all audio. Hooks
/// must never call it; use `POST /sound/stop` to quiet everything but keep rows.
pub async fn stop_handler(state: web::Data<ServerState>) -> impl Responder {
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  current.active.clear();
  current.audio_id = None;
  current.snoozed_until = None;
  current.waiting_tools.clear();
  state.audio.stop_all();
  HttpResponse::Ok().body("ok\n")
}
