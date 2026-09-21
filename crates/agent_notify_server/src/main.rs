//! Tiny local HTTP server that plays configured notification sounds.
//!
//! Default bind address: `127.0.0.1:43110`. Override with `HTTP_BIND_ADDRESS`.
//!
//! Audio playback runs on a dedicated OS thread (see [`audio_player`]). The
//! actix server itself is single-worker — we expect only the local agent to
//! call it. On SIGINT we tell actix to skip its grace period and we ship a
//! `Shutdown` command to the audio thread so any in-progress sound is dropped
//! immediately.

use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use actix_web::middleware::Logger;
use actix_web::web::Data;
use actix_web::{web, App, HttpServer};

use crate::audio_player::spawn_audio_player;
use crate::config::{NotifyConfig, DEFAULT_CONFIG_PATH};
use crate::endpoints::alert_handlers::{
  alert_await_handler, alert_beep_handler, alert_done_handler,
};
use crate::endpoints::loop_handlers::{loop_await_handler, loop_beep_handler, loop_done_handler};
use crate::endpoints::notification_handlers::*;
use crate::endpoints::root_handler::root_handler;
use crate::endpoints::state_handler::state_handler;
use crate::endpoints::stop_handler::stop_handler;
use crate::server_state::ServerState;

pub mod audio_player;
pub mod config;
pub mod endpoints;
#[cfg(test)]
mod notification_tests;
pub mod notifications;
pub mod server_state;

const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:43110";
const DEFAULT_RUST_LOG: &str = "info,actix_web=info";

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
  if env::var_os("RUST_LOG").is_none() {
    // SAFETY: single-threaded at this point — no other thread can read env.
    unsafe {
      env::set_var("RUST_LOG", DEFAULT_RUST_LOG);
    }
  }
  env_logger::init();

  let bind_address =
    env::var("HTTP_BIND_ADDRESS").unwrap_or_else(|_| DEFAULT_BIND_ADDRESS.to_string());

  let config_path: PathBuf = env::var("NOTIFY_CONFIG_PATH")
    .map(PathBuf::from)
    .unwrap_or_else(|_| PathBuf::from(DEFAULT_CONFIG_PATH));

  let config = Arc::new(NotifyConfig::read_from_file_or_default(&config_path));

  let (audio_handle, audio_thread) = spawn_audio_player();

  let state = ServerState {
    config: config.clone(),
    audio: audio_handle.clone(),
    notifications: Arc::new(Mutex::new(Default::default())),
  };

  log::info!("agent-notify-server listening on http://{}", bind_address);

  let server = HttpServer::new(move || {
    App::new()
      .app_data(Data::new(state.clone()))
      .app_data(web::JsonConfig::default().limit(32 * 1024))
      .wrap(
        Logger::default()
          .exclude("/notification")
          .exclude("/notifications")
          .exclude("/desktop/status"),
      )
      .route("/", web::get().to(root_handler))
      .route("/alert_beep", web::get().to(alert_beep_handler))
      .route("/alert_done", web::get().to(alert_done_handler))
      .route("/alert_await", web::get().to(alert_await_handler))
      .route("/loop_beep", web::get().to(loop_beep_handler))
      .route("/loop_done", web::get().to(loop_done_handler))
      .route("/loop_await", web::get().to(loop_await_handler))
      .route("/stop", web::get().to(stop_handler))
      .route("/state", web::get().to(state_handler))
      .route("/health", web::get().to(health))
      .route("/awaiting_user_input", web::post().to(awaiting_user_input))
      .route("/all_tasks_finished", web::post().to(all_tasks_finished))
      .route("/notification", web::get().to(current_notification))
      .route("/notifications", web::get().to(list_notifications))
      .route("/dismiss/{id}", web::post().to(dismiss))
      .route("/silence/{id}", web::post().to(silence))
      .route("/desktop/status", web::post().to(desktop_status))
      .route("/stop", web::post().to(stop_handler))
  })
  .bind(&bind_address)?;
  notifications::launch_desktop_app(server.addrs()[0]);
  server.workers(1).shutdown_timeout(0).run().await?;

  log::info!("server stopped; shutting down audio engine");
  audio_handle.shutdown();
  if let Err(e) = audio_thread.join() {
    log::warn!("audio thread join failed: {:?}", e);
  }
  Ok(())
}
