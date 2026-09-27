use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Notification {
  pub id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub session_id: Option<String>,
  #[serde(default)]
  pub silenced: bool,
  pub kind: String,
  pub title: String,
  pub message: String,
  #[serde(default, skip_serializing_if = "SessionContext::is_empty")]
  pub context: SessionContext,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub origin: Option<Origin>,
}

/// Global sound controls shared by every row. Per-row `silenced` flags feed the
/// one shared loop; a snooze mutes that loop until a recorded wall-clock time.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SoundState {
  /// When the snooze ends (RFC 3339 on the wire); `None` when not snoozed.
  pub snoozed_until: Option<DateTime<Utc>>,
  /// Whether any row still wants sound (it may currently be snoozed).
  pub alerting: bool,
}

/// Longest accepted snooze. Also bounds how far a backwards wall-clock jump can
/// extend an existing snooze.
pub const MAX_SNOOZE: TimeDelta = TimeDelta::hours(24);

/// Optional descriptive context, independent of the notification's immediate message.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SessionContext {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub cwd: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub work_arc: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub current_ask: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub repo_name: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub repo_description: Option<String>,
}

impl SessionContext {
  pub fn is_empty(&self) -> bool {
    self == &Self::default()
  }

  pub fn validate(&self) -> Result<(), &'static str> {
    for (value, limit) in [
      (&self.cwd, 4096),
      (&self.work_arc, 2000),
      (&self.current_ask, 2000),
      (&self.repo_name, 200),
      (&self.repo_description, 1000),
    ] {
      if value.as_ref().is_some_and(|text| {
        text.chars().count() > limit
          || text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
      }) {
        return Err(
          "context fields exceed their character limit or contain unsupported control characters",
        );
      }
    }
    Ok(())
  }

  /// Omitted/null fields retain previous values; empty strings explicitly clear them.
  pub fn apply(&mut self, patch: Self) {
    if patch
      .cwd
      .as_ref()
      .is_some_and(|cwd| Some(cwd.trim()) != self.cwd.as_deref())
    {
      // A different workspace must not inherit the previous repository's identity.
      self.repo_name = None;
      self.repo_description = None;
    }
    for (current, incoming) in [
      (&mut self.cwd, patch.cwd),
      (&mut self.work_arc, patch.work_arc),
      (&mut self.current_ask, patch.current_ask),
      (&mut self.repo_name, patch.repo_name),
      (&mut self.repo_description, patch.repo_description),
    ] {
      if let Some(value) = incoming {
        *current = (!value.trim().is_empty()).then(|| value.trim().to_owned());
      }
    }
  }
}

/// Hints, in decreasing precision, for returning to the requesting session.
/// Every field is optional; a plain title/message request remains valid.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Origin {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub terminal_app: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub app_pid: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub pid: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub terminal_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tty: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub window_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub window_title: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tmux_socket: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tmux_pane: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tmux_client: Option<String>,
}

impl Origin {
  pub fn validate(&self) -> Result<(), &'static str> {
    for value in [
      &self.terminal_app,
      &self.terminal_id,
      &self.tty,
      &self.window_id,
      &self.window_title,
      &self.tmux_socket,
      &self.tmux_pane,
      &self.tmux_client,
    ]
    .into_iter()
    .flatten()
    {
      if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        return Err(
          "origin strings must be nonblank, at most 1024 bytes, and contain no control characters",
        );
      }
    }
    if [self.pid, self.app_pid]
      .into_iter()
      .flatten()
      .any(|pid| pid <= 1 || pid > i32::MAX as u32)
    {
      return Err("origin process IDs must be between 2 and 2147483647");
    }
    if self.terminal_app.as_ref().is_some_and(|app| {
      !app
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b".-_".contains(&c))
    }) {
      return Err("terminal_app must be a terminal alias or application bundle identifier");
    }
    if self.tmux_pane.as_ref().is_some_and(|pane| {
      !pane
        .strip_prefix('%')
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit()))
    }) {
      return Err("tmux_pane must be a pane ID such as %101");
    }
    if self
      .tmux_socket
      .as_ref()
      .is_some_and(|path| !path.starts_with('/'))
    {
      return Err("tmux_socket must be an absolute path");
    }
    for tty in [&self.tty, &self.tmux_client].into_iter().flatten() {
      if !tty.starts_with("/dev/") || tty.contains("..") {
        return Err("tty and tmux_client must be /dev/ terminal paths");
      }
    }
    Ok(())
  }

  pub fn bundle_id(&self) -> Option<&str> {
    self.terminal_app.as_deref().map(|app| match app {
      "ghostty" | "Ghostty" => "com.mitchellh.ghostty",
      "iterm2" | "iTerm2" | "iTerm.app" => "com.googlecode.iterm2",
      "terminal" | "Terminal" | "Apple_Terminal" => "com.apple.Terminal",
      other => other,
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn legacy_and_partial_origins_are_valid() {
    let old: Notification =
      serde_json::from_str(r#"{"id":"a","kind":"done","title":"t","message":"m"}"#).unwrap();
    assert!(old.origin.is_none());
    for json in [
      "{}",
      r#"{"terminal_app":"ghostty"}"#,
      r#"{"pid":123}"#,
      r#"{"window_id":"42"}"#,
      r#"{"tmux_pane":"%1"}"#,
    ] {
      serde_json::from_str::<Origin>(json)
        .unwrap()
        .validate()
        .unwrap();
    }
  }

  #[test]
  fn rejects_unsafe_or_unbounded_hints() {
    for json in [
      r#"{"pid":0}"#,
      r#"{"pid":4294967295}"#,
      r#"{"tmux_pane":"%1; kill-server"}"#,
      r#"{"tmux_socket":"relative"}"#,
      r#"{"tty":"/tmp/tty"}"#,
      r#"{"window_title":"x\ny"}"#,
    ] {
      let origin: Origin = serde_json::from_str(json).unwrap();
      assert!(origin.validate().is_err(), "{json}");
    }
  }

  #[test]
  fn sound_state_uses_rfc3339_timestamps_and_tolerates_older_servers() {
    let until = DateTime::parse_from_rfc3339("2026-09-27T15:04:05Z")
      .unwrap()
      .with_timezone(&Utc);
    let state = SoundState {
      snoozed_until: Some(until),
      alerting: true,
    };
    let json = serde_json::to_string(&state).unwrap();
    assert_eq!(
      json,
      r#"{"snoozed_until":"2026-09-27T15:04:05Z","alerting":true}"#
    );
    assert_eq!(serde_json::from_str::<SoundState>(&json).unwrap(), state);
    assert_eq!(
      serde_json::from_str::<SoundState>("{}").unwrap(),
      SoundState::default()
    );
  }
}
