//! Best-effort backup of task state, so rows survive a service restart (but
//! not a reboot: the file lives under /tmp, which macOS clears on boot).
//!
//! The in-memory `NotificationState` is the only source of truth. A backup is
//! read once, at startup, as a seed for a fresh process, and every restored row
//! is validated again. Nothing here may block startup, a request, or shutdown:
//! each failure is logged and ignored.
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::notifications::{Notification, NotificationState};

/// Bump when the layout changes incompatibly; other versions are ignored.
const FORMAT: u32 = 1;
/// How often a running service refreshes its backup. SIGKILL cannot be caught,
/// so this bounds what a hard kill can lose.
pub const INTERVAL: Duration = Duration::from_secs(5 * 60);
/// The longest shutdown waits for the final backup before exiting anyway.
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(1);

/// One backup per port, so separate service instances never share a file.
pub fn default_path(port: u16) -> PathBuf {
  PathBuf::from("/tmp/desktop-notify").join(format!("state-{port}.toml"))
}

/// What a restart needs. The audio engine, desktop heartbeat, and anything
/// derived from rows are rebuilt by the new process instead.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Backup {
  format: u32,
  saved_at: DateTime<Utc>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  snoozed_until: Option<DateTime<Utc>>,
  /// Session ID → the tool call its `input_needed` row waits on.
  #[serde(default)]
  waiting_tools: HashMap<String, String>,
  #[serde(default)]
  notifications: Vec<Notification>,
}

impl Backup {
  /// Copies the state; call with the lock held, then write after releasing it.
  pub fn of(state: &NotificationState) -> Self {
    Self {
      format: FORMAT,
      saved_at: Utc::now(),
      snoozed_until: state.snoozed_until,
      waiting_tools: state.waiting_tools.clone(),
      notifications: state.active.clone(),
    }
  }

  /// Moves the backed-up rows into a fresh state, keeping only rows that pass
  /// the same checks a request would, one per session.
  pub fn restore_into(self, state: &mut NotificationState) {
    let mut sessions = std::collections::HashSet::new();
    let rows: Vec<Notification> = self
      .notifications
      .into_iter()
      .filter(|row| valid_row(row) && sessions.insert(row.session_id.clone()))
      .collect();
    state.waiting_tools = self
      .waiting_tools
      .into_iter()
      .filter(|(session, tool)| {
        valid_id(session) && valid_id(tool) && sessions.contains(&Some(session.clone()))
      })
      .collect();
    state.snoozed_until = self.snoozed_until;
    state.active = rows;
  }

  /// Equal apart from when it was taken.
  fn same_content(&self, other: &Self) -> bool {
    self.notifications == other.notifications
      && self.snoozed_until == other.snoozed_until
      && self.waiting_tools == other.waiting_tools
  }
}

fn valid_id(id: &str) -> bool {
  !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}

fn valid_row(row: &Notification) -> bool {
  let text = |value: &str, limit: usize| !value.is_empty() && value.chars().count() <= limit;
  row.id.len() == 32
    && row.id.bytes().all(|c| c.is_ascii_hexdigit())
    && row.session_id.as_deref().is_none_or(valid_id)
    && text(&row.title, 200)
    && text(&row.message, 4000)
    && row.context.validate().is_ok()
    && row
      .origin
      .as_ref()
      .is_none_or(|origin| origin.validate().is_ok())
}

/// Reads a backup, or `None` for anything missing, unreadable, from another
/// format, or not private to this user (it is in a shared directory).
pub fn load(path: &Path) -> Option<Backup> {
  let private = |path: &Path| {
    fs::symlink_metadata(path)
      .is_ok_and(|meta| !meta.file_type().is_symlink() && meta.permissions().mode() & 0o077 == 0)
  };
  if !path.parent().is_some_and(private) || !private(path) {
    return None;
  }
  let text = fs::read_to_string(path)
    .map_err(|e| log::warn!("ignoring task backup {}: {e}", path.display()))
    .ok()?;
  toml::from_str::<Backup>(&text)
    .map_err(|e| log::warn!("ignoring unreadable task backup {}: {e}", path.display()))
    .ok()
    .filter(|backup| backup.format == FORMAT)
}

/// Writes atomically (temporary file, then rename) into a private directory.
pub fn write(path: &Path, backup: &Backup) -> std::io::Result<()> {
  let text = toml::to_string(backup).map_err(std::io::Error::other)?;
  let directory = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
  match fs::DirBuilder::new().mode(0o700).create(directory) {
    Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
    _ => {}
  }
  let temporary = path.with_extension("toml.tmp");
  let _ = fs::remove_file(&temporary);
  let mut file = fs::OpenOptions::new()
    .write(true)
    .create_new(true)
    .mode(0o600)
    .open(&temporary)?;
  file.write_all(text.as_bytes())?;
  drop(file);
  fs::rename(&temporary, path)
}

/// Writes on a helper thread and waits at most `deadline`, so a stuck
/// filesystem can never hold up shutdown.
pub fn write_with_deadline(path: PathBuf, backup: Backup, deadline: Duration) {
  let (done, finished) = mpsc::channel();
  std::thread::spawn(move || {
    let _ = done.send(write(&path, &backup).map(|()| path));
  });
  match finished.recv_timeout(deadline) {
    Ok(Ok(path)) => log::info!("backed up tasks to {}", path.display()),
    Ok(Err(e)) => log::warn!("could not back up tasks: {e}"),
    Err(_) => log::warn!("task backup timed out; exiting without it"),
  }
}

/// Refreshes the backup every `INTERVAL` on a detached thread, skipping writes
/// when nothing changed. It dies with the process.
pub fn spawn_periodic(path: PathBuf, state: std::sync::Arc<std::sync::Mutex<NotificationState>>) {
  std::thread::spawn(move || {
    let mut last: Option<Backup> = None;
    loop {
      std::thread::sleep(INTERVAL);
      let backup = Backup::of(&state.lock().unwrap_or_else(|e| e.into_inner()));
      if last.as_ref().is_some_and(|last| last.same_content(&backup)) {
        continue;
      }
      match write(&path, &backup) {
        Ok(()) => last = Some(backup),
        Err(e) => log::warn!("could not back up tasks to {}: {e}", path.display()),
      }
    }
  });
}

#[cfg(test)]
mod tests {
  use super::*;
  use notify_types::TaskState;

  fn directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
      "desktop-notify-test-{name}-{}-{:x}",
      std::process::id(),
      rand::random::<u64>()
    ));
    let _ = fs::remove_dir_all(&path);
    path
  }

  fn row(id: char, session: &str, state: TaskState) -> Notification {
    serde_json::from_value(serde_json::json!({
      "id": id.to_string().repeat(32), "session_id": session, "state": state,
      "agent": "codex", "title": "Task \"quoted\" ✓", "message": "Line one\nline two",
      "context": {"cwd": "/work", "repo_name": "demo"},
      "origin": {"terminal_app": "ghostty", "pid": 42, "tmux_pane": "%3"},
      "times": {"tracked_since": "2026-09-27T10:00:00.123456789Z", "task_started_at": "2026-09-27T10:00:00Z"},
    }))
    .unwrap()
  }

  #[test]
  fn round_trips_rows_snooze_and_waiting_tools_through_a_private_file() {
    let dir = directory("roundtrip");
    let path = dir.join("state-1.toml");
    let mut state = NotificationState::default();
    state.active = vec![
      row('a', "one", TaskState::InputNeeded),
      row('b', "two", TaskState::DoneAcknowledged),
    ];
    state.snoozed_until = Some(Utc::now());
    state.waiting_tools.insert("one".into(), "tool-1".into());
    write(&path, &Backup::of(&state)).unwrap();
    assert_eq!(
      fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
      0o700
    );
    assert_eq!(
      fs::metadata(&path).unwrap().permissions().mode() & 0o777,
      0o600
    );
    let mut restored = NotificationState::default();
    load(&path).unwrap().restore_into(&mut restored);
    assert_eq!(restored.active, state.active);
    assert_eq!(restored.snoozed_until, state.snoozed_until);
    assert_eq!(restored.waiting_tools, state.waiting_tools);
    // Overwriting replaces the previous backup.
    state.active.truncate(1);
    write(&path, &Backup::of(&state)).unwrap();
    assert_eq!(load(&path).unwrap().notifications.len(), 1);
    fs::remove_dir_all(dir).unwrap();
  }

  #[test]
  fn missing_corrupt_foreign_or_shared_backups_are_ignored() {
    let dir = directory("ignored");
    let path = dir.join("state-1.toml");
    assert!(load(&path).is_none());
    write(&path, &Backup::of(&NotificationState::default())).unwrap();
    assert!(load(&path).is_some());
    fs::write(&path, "not = [valid").unwrap();
    assert!(load(&path).is_none());
    fs::write(&path, "format = 99\nsaved_at = 2026-09-27T10:00:00Z\n").unwrap();
    assert!(load(&path).is_none());
    write(&path, &Backup::of(&NotificationState::default())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load(&path).is_none());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(load(&path).is_none());
    fs::remove_dir_all(dir).unwrap();
    // An unwritable location is an error for the caller to log, not a panic.
    assert!(write(
      Path::new("/nonexistent/dir/state.toml"),
      &Backup::of(&NotificationState::default())
    )
    .is_err());
  }

  #[test]
  fn restoring_drops_invalid_rows_duplicates_and_orphaned_waiting_tools() {
    let mut bad_id = row('c', "three", TaskState::Done);
    bad_id.id = "not-hex".into();
    let mut blank = row('d', "four", TaskState::Done);
    blank.title.clear();
    let backup = Backup {
      format: FORMAT,
      saved_at: Utc::now(),
      snoozed_until: None,
      waiting_tools: HashMap::from([
        ("one".into(), "tool".into()),
        ("gone".into(), "tool".into()),
      ]),
      notifications: vec![
        row('a', "one", TaskState::InputNeeded),
        row('b', "one", TaskState::Done),
        bad_id,
        blank,
      ],
    };
    let mut state = NotificationState::default();
    backup.restore_into(&mut state);
    assert_eq!(state.active.len(), 1);
    assert_eq!(state.active[0].id, "a".repeat(32));
    assert_eq!(state.waiting_tools.len(), 1);
    assert!(state.waiting_tools.contains_key("one"));
  }

  #[test]
  fn a_stuck_write_cannot_delay_shutdown_past_its_deadline() {
    // Shutdown returns once the deadline passes, whether or not the helper
    // thread has finished (or failed) writing.
    let dir = directory("deadline");
    let started = std::time::Instant::now();
    write_with_deadline(
      dir.join("state-1.toml"),
      Backup::of(&NotificationState::default()),
      Duration::ZERO,
    );
    write_with_deadline(
      PathBuf::from("/nonexistent/state.toml"),
      Backup::of(&NotificationState::default()),
      Duration::from_millis(200),
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    let _ = fs::remove_dir_all(dir);
  }
}
