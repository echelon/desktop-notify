use serde::Serialize;

pub use notify_types::{Notification, SoundState};

#[derive(Clone, Default, Serialize, PartialEq, Eq)]
pub struct Snapshot {
  pub notifications: Vec<Notification>,
  /// Global sound controls; defaults when an older service lacks `/sound`.
  pub sound: SoundState,
  /// Base URL of the service, shown so the app documents its REST interface.
  pub service: String,
  pub connected: bool,
  pub error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum VisibilityChange {
  Show,
  Hide,
  Keep,
}

/// New/replaced alerting rows open the window; a row that only became busy
/// (`working`) or quiet does not. Clearing one row cannot hide the others.
/// `next` pairs each ID with whether its row is alerting.
pub fn visibility_change(previous: &[&str], next: &[(&str, bool)]) -> VisibilityChange {
  if next
    .iter()
    .any(|(id, alerting)| *alerting && !previous.contains(id))
  {
    VisibilityChange::Show
  } else if !previous.is_empty() && next.is_empty() {
    VisibilityChange::Hide
  } else {
    VisibilityChange::Keep
  }
}

/// The tray bell swings while the alert sound should be playing: some row is
/// alerting, and it is neither snoozed nor unreachable. Offline, the retained
/// rows may be stale and the service's sound has stopped with it.
pub fn bell_swings(snapshot: &Snapshot) -> bool {
  snapshot.connected
    && snapshot.sound.snoozed_until.is_none()
    && snapshot.notifications.iter().any(|n| n.state.is_alerting())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn snapshot(states: &[&str], snoozed_until: Option<&str>, connected: bool) -> Snapshot {
    let notifications = states
      .iter()
      .enumerate()
      .map(|(i, state)| {
        serde_json::from_value(serde_json::json!({
          "id": i.to_string(), "state": state, "title": "t", "message": "m",
        }))
        .unwrap()
      })
      .collect();
    let sound = serde_json::from_value(serde_json::json!({
      "snoozed_until": snoozed_until, "alerting": !states.is_empty(),
    }))
    .unwrap();
    Snapshot {
      notifications,
      sound,
      connected,
      ..Snapshot::default()
    }
  }

  #[test]
  fn the_bell_swings_for_any_alerting_row() {
    for state in ["input_needed", "done", "failed"] {
      assert!(bell_swings(&snapshot(&["working", state], None, true)));
    }
  }

  #[test]
  fn the_bell_rests_when_nothing_needs_attention() {
    assert!(!bell_swings(&snapshot(&[], None, true)));
    assert!(!bell_swings(&snapshot(
      &[
        "working",
        "input_needed_ignored",
        "done_acknowledged",
        "failed_acknowledged"
      ],
      None,
      true
    )));
  }

  #[test]
  fn the_bell_rests_while_snoozed_or_offline() {
    let until = Some("2026-09-27T15:04:05Z");
    assert!(!bell_swings(&snapshot(&["done"], until, true)));
    assert!(!bell_swings(&snapshot(&["done"], None, false)));
  }

  #[test]
  fn new_and_replacement_alerts_open_the_window() {
    assert_eq!(
      visibility_change(&[], &[("a", true)]),
      VisibilityChange::Show
    );
    assert_eq!(
      visibility_change(&["a"], &[("b", true), ("a", true)]),
      VisibilityChange::Show
    );
    assert_eq!(
      visibility_change(&["a", "b"], &[("c", true), ("b", true)]),
      VisibilityChange::Show
    );
  }

  #[test]
  fn unchanged_or_reordered_alerts_preserve_visibility() {
    assert_eq!(
      visibility_change(&["a"], &[("a", true)]),
      VisibilityChange::Keep
    );
    assert_eq!(
      visibility_change(&["a", "b"], &[("b", true), ("a", true)]),
      VisibilityChange::Keep
    );
  }

  #[test]
  fn quiet_or_busy_updates_do_not_open_the_window() {
    assert_eq!(
      visibility_change(&["a"], &[("b", false), ("c", true)]),
      VisibilityChange::Show
    );
    assert_eq!(
      visibility_change(&["a"], &[("b", false)]),
      VisibilityChange::Keep
    );
    assert_eq!(
      visibility_change(&[], &[("w", false)]),
      VisibilityChange::Keep
    );
  }

  #[test]
  fn removing_one_row_keeps_the_other_visible() {
    assert_eq!(
      visibility_change(&["a", "b"], &[("b", true)]),
      VisibilityChange::Keep
    );
    assert_eq!(visibility_change(&["a"], &[]), VisibilityChange::Hide);
    assert_eq!(visibility_change(&[], &[]), VisibilityChange::Keep);
  }
}
