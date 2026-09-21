//! The active alert is shared by the REST API and the Tauri tray app.
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub use notify_types::Notification;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DesktopStatus {
  pub presentation: String,
  pub window_visible: bool,
  pub pid: u32,
  pub displayed_id: Option<String>,
  #[serde(default)]
  pub displayed_ids: Vec<String>,
  pub error: Option<String>,
}

#[derive(Default)]
pub struct NotificationState {
  /// Most recently updated first; one entry per session (None is the legacy slot).
  pub active: Vec<Notification>,
  pub audio_id: Option<String>,
  pub desktop: DesktopStatus,
  pub desktop_seen: Option<Instant>,
}

impl NotificationState {
  pub fn desktop_connected(&self) -> bool {
    self
      .desktop_seen
      .is_some_and(|seen| seen.elapsed().as_secs() < 15)
  }
}

/// Launch Services reuses the running tray app across service restarts.
#[cfg(target_os = "macos")]
pub fn launch_desktop_app(address: std::net::SocketAddr) {
  if std::env::var_os("NOTIFY_DISABLE_DESKTOP").is_some() {
    return;
  }
  let default = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../target/desktop/Desktop Notify.app"
  );
  let app = std::env::var("NOTIFY_APP_PATH").unwrap_or_else(|_| default.into());
  if !std::path::Path::new(&app).is_dir() {
    log::warn!("Tauri app missing; run python3 scripts/build.py");
    return;
  }
  let host = if address.is_ipv6() {
    "[::1]"
  } else {
    "127.0.0.1"
  };
  let url = format!("http://{}:{}", host, address.port());
  let mut command = std::process::Command::new("/usr/bin/open");
  command.args(["-g", &app, "--args", "--server-url", &url]);
  match command.status() {
    Ok(status) if status.success() => {}
    result => log::warn!("could not launch Tauri tray app: {:?}", result),
  }
}

#[cfg(not(target_os = "macos"))]
pub fn launch_desktop_app(_address: std::net::SocketAddr) {}
