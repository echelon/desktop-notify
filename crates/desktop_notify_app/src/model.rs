use serde::Serialize;

pub use notify_types::{Notification, SoundState};

#[derive(Clone, Default, Serialize, PartialEq, Eq)]
pub struct Snapshot {
  pub notifications: Vec<Notification>,
  /// Global sound controls; defaults when an older service lacks `/sound`.
  pub sound: SoundState,
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

#[cfg(test)]
mod tests {
  use super::*;

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
