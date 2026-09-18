use actix_web::{web, HttpResponse, Responder};

use crate::server_state::ServerState;

pub async fn stop_handler(state: web::Data<ServerState>) -> impl Responder {
  state.audio.stop_all();
  HttpResponse::Ok().body("ok\n")
}
