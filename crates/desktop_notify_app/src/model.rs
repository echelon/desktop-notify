use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Notification {
  pub id: String,
  pub kind: String,
  pub title: String,
  pub message: String,
}

#[derive(Clone, Default, Serialize, PartialEq, Eq)]
pub struct Snapshot {
  pub notification: Option<Notification>,
  pub connected: bool,
  pub error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum VisibilityChange {
  Show,
  Hide,
  Keep,
}

/// Only a new alert opens the window. Polling must never undo a user's hide.
pub fn visibility_change(previous: Option<&str>, next: Option<&str>) -> VisibilityChange {
  match (previous, next) {
    (a, Some(b)) if a != Some(b) => VisibilityChange::Show,
    (Some(_), None) => VisibilityChange::Hide,
    _ => VisibilityChange::Keep,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn new_and_replacement_alerts_open_the_window() {
    assert_eq!(visibility_change(None, Some("a")), VisibilityChange::Show);
    assert_eq!(
      visibility_change(Some("a"), Some("b")),
      VisibilityChange::Show
    );
  }

  #[test]
  fn hidden_alert_stays_hidden_until_replaced_or_recalled() {
    assert_eq!(
      visibility_change(Some("a"), Some("a")),
      VisibilityChange::Keep
    );
  }

  #[test]
  fn dismissal_hides_but_idle_tray_recall_stays_open() {
    assert_eq!(visibility_change(Some("a"), None), VisibilityChange::Hide);
    assert_eq!(visibility_change(None, None), VisibilityChange::Keep);
  }
}
