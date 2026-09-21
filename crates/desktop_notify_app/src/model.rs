use serde::Serialize;

pub use notify_types::Notification;

#[derive(Clone, Default, Serialize, PartialEq, Eq)]
pub struct Snapshot {
  pub notifications: Vec<Notification>,
  pub connected: bool,
  pub error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum VisibilityChange {
  Show,
  Hide,
  Keep,
}

/// New/replaced alerts open the window; clearing one row cannot hide the others.
pub fn visibility_change(previous: &[&str], next: &[&str]) -> VisibilityChange {
  if next.iter().any(|id| !previous.contains(id)) {
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
    assert_eq!(visibility_change(&[], &["a"]), VisibilityChange::Show);
    assert_eq!(
      visibility_change(&["a"], &["b", "a"]),
      VisibilityChange::Show
    );
    assert_eq!(
      visibility_change(&["a", "b"], &["c", "b"]),
      VisibilityChange::Show
    );
  }

  #[test]
  fn unchanged_or_reordered_alerts_preserve_visibility() {
    assert_eq!(visibility_change(&["a"], &["a"]), VisibilityChange::Keep);
    assert_eq!(
      visibility_change(&["a", "b"], &["b", "a"]),
      VisibilityChange::Keep
    );
  }

  #[test]
  fn removing_one_row_keeps_the_other_visible() {
    assert_eq!(
      visibility_change(&["a", "b"], &["b"]),
      VisibilityChange::Keep
    );
    assert_eq!(visibility_change(&["a"], &[]), VisibilityChange::Hide);
    assert_eq!(visibility_change(&[], &[]), VisibilityChange::Keep);
  }
}
