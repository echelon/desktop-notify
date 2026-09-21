use actix_web::{web, HttpResponse, Responder};

use crate::server_state::ServerState;

pub async fn stop_handler(state: web::Data<ServerState>) -> impl Responder {
  let mut current = state
    .notifications
    .lock()
    .unwrap_or_else(|e| e.into_inner());
  current.active.clear();
  current.audio_id = None;
  state.audio.stop_all();
  HttpResponse::Ok().body("ok\n")
}
