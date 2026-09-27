use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

/// Lifecycle of one agent session's row. Alerting states feed the one shared
/// sound loop; dismissing a row moves it to its quiet counterpart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
  /// The agent is busy (a prompt was submitted or it resumed after input).
  Working,
  /// A question or permission request is waiting for the user.
  InputNeeded,
  /// Still waiting, but the user chose to stop being alerted.
  InputNeededIgnored,
  /// The turn finished and has not been acknowledged.
  Done,
  /// The user has seen the finished turn.
  DoneAcknowledged,
  /// The turn ended on an error (for example an API or rate-limit failure).
  Failed,
  /// The user has seen the failure.
  FailedAcknowledged,
}

impl TaskState {
  pub fn is_alerting(self) -> bool {
    matches!(self, Self::InputNeeded | Self::Done | Self::Failed)
  }

  /// Where a dismissal takes this state; `None` when it is already quiet.
  pub fn acknowledged(self) -> Option<Self> {
    match self {
      Self::InputNeeded => Some(Self::InputNeededIgnored),
      Self::Done => Some(Self::DoneAcknowledged),
      Self::Failed => Some(Self::FailedAcknowledged),
      _ => None,
    }
  }

  pub fn is_waiting(self) -> bool {
    matches!(self, Self::InputNeeded | Self::InputNeededIgnored)
  }
}

/// The coding agent that reported a row, when the producer knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
  ClaudeCode,
  Codex,
  /// A value from a newer producer; kept so an older consumer still accepts the row.
  #[serde(other)]
  Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Notification {
  pub id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub session_id: Option<String>,
  pub state: TaskState,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub agent: Option<Agent>,
  pub title: String,
  pub message: String,
  #[serde(default, skip_serializing_if = "SessionContext::is_empty")]
  pub context: SessionContext,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub origin: Option<Origin>,
}

/// Global sound controls shared by every row. Alerting rows feed the one
/// shared loop; a snooze mutes that loop until a recorded wall-clock time.
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
  /// Ghostty tab containing `terminal_id` (informational; focus uses the terminal).
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tab_id: Option<String>,
  /// tmux server process. Pane IDs restart with the server, so focus refuses a
  /// pane whose socket is now served by a different process.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tmux_server_pid: Option<u32>,
  /// Stable tmux session ID such as `$3`.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tmux_session: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tmux_session_name: Option<String>,
  /// Stable tmux window ID such as `@12`.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tmux_window: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tmux_window_index: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tmux_window_name: Option<String>,
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
      &self.tab_id,
      &self.tmux_session,
      &self.tmux_session_name,
      &self.tmux_window,
      &self.tmux_window_name,
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
    if [self.pid, self.app_pid, self.tmux_server_pid]
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
    for (value, prefix, error) in [
      (
        &self.tmux_session,
        '$',
        "tmux_session must be a session ID such as $3",
      ),
      (
        &self.tmux_window,
        '@',
        "tmux_window must be a window ID such as @12",
      ),
    ] {
      if value.as_ref().is_some_and(|id| {
        !id
          .strip_prefix(prefix)
          .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
      }) {
        return Err(error);
      }
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
      serde_json::from_str(r#"{"id":"a","state":"done","title":"t","message":"m"}"#).unwrap();
    assert!(old.origin.is_none());
    for json in [
      "{}",
      r#"{"terminal_app":"ghostty"}"#,
      r#"{"pid":123}"#,
      r#"{"window_id":"42"}"#,
      r#"{"tmux_pane":"%1"}"#,
      r#"{"tab_id":"tab-1","tmux_server_pid":6330,"tmux_session":"$32","tmux_session_name":"work","tmux_window":"@7","tmux_window_index":0,"tmux_window_name":"claude"}"#,
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
      r#"{"tmux_session":"work"}"#,
      r#"{"tmux_window":"@1; kill-server"}"#,
      r#"{"tmux_server_pid":1}"#,
      r#"{"tmux_window_name":"a\u0007b"}"#,
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

  #[test]
  fn task_states_serialize_as_snake_case_and_acknowledge_to_quiet_states() {
    let json = |state| serde_json::to_string(&state).unwrap();
    assert_eq!(
      json(TaskState::InputNeededIgnored),
      r#""input_needed_ignored""#
    );
    assert_eq!(json(TaskState::DoneAcknowledged), r#""done_acknowledged""#);
    for state in [TaskState::InputNeeded, TaskState::Done, TaskState::Failed] {
      assert!(state.is_alerting());
      assert!(!state.acknowledged().unwrap().is_alerting());
    }
    assert_eq!(TaskState::Working.acknowledged(), None);
    assert!(TaskState::InputNeededIgnored.is_waiting());
  }

  #[test]
  fn agent_is_optional_and_tolerates_newer_values() {
    let row = |extra: &str| {
      serde_json::from_str::<Notification>(&format!(
        r#"{{"id":"a","state":"done","title":"t","message":"m"{extra}}}"#
      ))
      .unwrap()
      .agent
    };
    assert_eq!(row(""), None);
    assert_eq!(row(r#","agent":"claude_code""#), Some(Agent::ClaudeCode));
    assert_eq!(row(r#","agent":"codex""#), Some(Agent::Codex));
    assert_eq!(row(r#","agent":"gemini_cli""#), Some(Agent::Unknown));
    assert_eq!(
      serde_json::to_string(&Agent::ClaudeCode).unwrap(),
      r#""claude_code""#
    );
  }
}
