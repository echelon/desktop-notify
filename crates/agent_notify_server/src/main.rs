//! Tiny local HTTP server that plays configured notification sounds.
//!
//! Default bind address: `127.0.0.1:43110`. Override with `HTTP_BIND_ADDRESS`.
//!
//! Audio playback runs on a dedicated OS thread (see [`audio_player`]). The
//! actix server itself is single-worker — we expect only the local agent to
//! call it. On SIGINT/SIGTERM we tell actix to skip its grace period, write a
//! best-effort task backup (bounded by a deadline; see [`backup`]), and ship a
//! `Shutdown` command to the audio thread so any in-progress sound is dropped
//! immediately. SIGKILL cannot be caught; the periodic backup covers it.

use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use actix_web::middleware::Logger;
use actix_web::web::Data;
use actix_web::{web, App, HttpServer};

use crate::audio_player::spawn_audio_player;
use crate::config::{NotifyConfig, DEFAULT_CONFIG_PATH};
use crate::endpoints::notification_handlers::*;
use crate::endpoints::root_handler::root_handler;
use crate::endpoints::state_handler::state_handler;
use crate::endpoints::stop_handler;
use crate::server_state::ServerState;

pub mod audio_player;
pub mod backup;
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
  // A convenience only: the backup seeds this fresh process, and live state is
  // authoritative from here on. Resolve the port before binding so a bind
  // failure (another instance owns the port) leaves its backup untouched.
  let backup_path = bind_address
    .parse::<std::net::SocketAddr>()
    .map(|address| backup::default_path(address.port()))
    .ok();
  if let Some(restored) = backup_path.as_deref().and_then(backup::load) {
    let mut current = state
      .notifications
      .lock()
      .unwrap_or_else(|e| e.into_inner());
    restored.restore_into(&mut current);
    log::info!("restored {} tasks from backup", current.active.len());
    // Alerting rows resume their sound, as they would have without a restart.
    reconcile_audio(&state, &mut current);
  }

  log::info!("agent-notify-server listening on http://{}", bind_address);

  // The shutdown backup reads the same shared state after the server stops.
  let app_state = state.clone();
  let server = HttpServer::new(move || {
    App::new()
      .app_data(Data::new(app_state.clone()))
      .app_data(web::JsonConfig::default().limit(32 * 1024))
      .wrap(
        Logger::default()
          .exclude("/notifications")
          .exclude("/sound")
          .exclude("/working")
          .exclude("/desktop/status"),
      )
      // Permanent: the web interface for managing tasks, alerts, and sound,
      // plus the live API reference. Never remove it.
      .route("/", web::get().to(root_handler))
      .configure(stop_handler::routes)
      .route("/state", web::get().to(state_handler))
      .route("/health", web::get().to(health))
      .route("/awaiting_user_input", web::post().to(awaiting_user_input))
      .route("/all_tasks_finished", web::post().to(all_tasks_finished))
      .route("/notifications", web::get().to(list_notifications))
      .route("/dismiss/{id}", web::post().to(dismiss))
      .route("/working", web::post().to(working))
      .route("/task_failed", web::post().to(task_failed))
      .route("/acknowledge/{id}", web::post().to(acknowledge))
      .route("/focused/{id}", web::post().to(focused))
      .route("/sound", web::get().to(sound))
      .route("/sound/snooze", web::post().to(snooze))
      .route("/sound/resume", web::post().to(resume_sound))
      .route("/desktop/status", web::post().to(desktop_status))
  })
  .bind(&bind_address)?;
  notifications::launch_desktop_app(server.addrs()[0]);
  if let Some(path) = &backup_path {
    backup::spawn_periodic(path.clone(), state.notifications.clone());
  }
  let result = server.workers(1).shutdown_timeout(0).run().await;

  if let Some(path) = backup_path {
    let snapshot = backup::Backup::of(
      &state
        .notifications
        .lock()
        .unwrap_or_else(|e| e.into_inner()),
    );
    backup::write_with_deadline(path, snapshot, backup::SHUTDOWN_DEADLINE);
  }
  result?;

  log::info!("server stopped; shutting down audio engine");
  audio_handle.shutdown();
  if let Err(e) = audio_thread.join() {
    log::warn!("audio thread join failed: {:?}", e);
  }
  Ok(())
}
