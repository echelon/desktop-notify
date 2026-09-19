//! All focus actions originate from a button click, never from receiving an alert.
use notify_types::Origin;
use serde::Serialize;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Serialize)]
pub struct FocusResult {
  pub target: String,
  pub tmux_focused: bool,
  pub warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
  Terminal(String),
  Tty(String),
  WindowId(String),
  WindowTitle(String),
  Application,
}

impl Target {
  fn name(&self) -> &'static str {
    match self {
      Self::Terminal(_) | Self::Tty(_) => "terminal",
      Self::WindowId(_) | Self::WindowTitle(_) => "window",
      Self::Application => "application",
    }
  }
}

fn targets(origin: &Origin) -> Vec<Target> {
  let mut steps = Vec::new();
  if let Some(id) = &origin.terminal_id {
    steps.push(Target::Terminal(id.clone()));
  }
  if let Some(tty) = &origin.tty {
    steps.push(Target::Tty(tty.clone()));
  }
  if let Some(id) = &origin.window_id {
    steps.push(Target::WindowId(id.clone()));
  }
  if let Some(title) = &origin.window_title {
    steps.push(Target::WindowTitle(title.clone()));
  }
  steps.push(Target::Application);
  steps
}

fn try_targets(
  origin: &Origin,
  mut attempt: impl FnMut(&Target) -> Result<(), String>,
) -> Result<FocusResult, String> {
  let mut failures = Vec::new();
  for target in targets(origin) {
    match attempt(&target) {
      Ok(()) => {
        return Ok(FocusResult {
          target: target.name().into(),
          tmux_focused: false,
          warning: (!failures.is_empty())
            .then(|| format!("Focused the {}. {}", target.name(), failures.join(" "))),
        })
      }
      Err(error) => {
        if !failures.contains(&error) {
          failures.push(error);
        }
      }
    }
  }
  Err(failures.join(" "))
}

/// Bound external helpers, including a pending macOS Automation prompt.
fn output(command: &mut Command) -> Result<String, String> {
  let mut child = command
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|e| e.to_string())?;
  let deadline = Instant::now() + Duration::from_secs(15);
  loop {
    if child.try_wait().map_err(|e| e.to_string())?.is_some() {
      break;
    }
    if Instant::now() >= deadline {
      let _ = child.kill();
      let _ = child.wait();
      return Err(
        "Focus timed out. Check System Settings → Privacy & Security → Automation, then retry."
          .into(),
      );
    }
    std::thread::sleep(Duration::from_millis(30));
  }
  let result = child.wait_with_output().map_err(|e| e.to_string())?;
  if !result.status.success() {
    let error = String::from_utf8_lossy(&result.stderr);
    if error.contains("-1743") || error.contains("-25211") || error.contains("1002") {
      return Err("Allow Desktop Notify to control your terminal in System Settings → Privacy & Security → Automation (or Accessibility for window-title matching).".into());
    }
    return Err(error.trim().chars().take(400).collect());
  }
  Ok(String::from_utf8_lossy(&result.stdout).trim().into())
}

fn tmux_program() -> String {
  for path in ["/opt/homebrew/bin/tmux", "/usr/local/bin/tmux"] {
    if std::path::Path::new(path).is_file() {
      return path.into();
    }
  }
  "tmux".into()
}

fn focus_tmux(origin: &Origin) -> Result<(), String> {
  let pane = origin
    .tmux_pane
    .as_deref()
    .ok_or("No tmux pane supplied.")?;
  // Never select an arbitrary server inherited by the desktop app.
  let socket = origin
    .tmux_socket
    .as_deref()
    .ok_or("No tmux socket supplied; focused the terminal only.")?;
  let run = |args: &[&str]| output(Command::new(tmux_program()).args(["-S", socket]).args(args));
  // Resolve before mutating; a closed pane must not select another target.
  let resolved = run(&["display-message", "-p", "-t", pane, "#{pane_id}"])?;
  if resolved != pane {
    return Err("The tmux pane is no longer available.".into());
  }
  if let Some(client) = &origin.tmux_client {
    run(&["switch-client", "-c", client, "-t", pane])?;
  } else {
    run(&["select-window", "-t", pane])?;
  }
  run(&["select-pane", "-t", pane])?;
  Ok(())
}

pub fn focus(origin: &Origin) -> Result<FocusResult, String> {
  origin.validate().map_err(str::to_owned)?;
  #[cfg(target_os = "macos")]
  {
    let apps = macos::applications(origin);
    if apps.is_empty() {
      return Err(
        "The requesting application is no longer running, or no application identity was supplied."
          .into(),
      );
    }
    let tmux = origin.tmux_pane.as_ref().map(|_| focus_tmux(origin));
    let mut result = try_targets(origin, |target| {
      if matches!(target, Target::Application) && apps.len() != 1 {
        return Err(
          "Several terminal apps are running; supply terminal_app or app_pid to focus one.".into(),
        );
      }
      let mut error = "The requested terminal or window is no longer available.".to_string();
      for app in &apps {
        match macos::attempt(app, target) {
          Ok(()) => return Ok(()),
          Err(e) => error = e,
        }
      }
      Err(error)
    })?;
    match tmux {
      Some(Ok(())) => result.tmux_focused = true,
      Some(Err(_)) => {
        let warning = "The tmux pane could not be selected; you can select it in the terminal.";
        result.warning = Some(match result.warning {
          Some(previous) => format!("{previous} {warning}"),
          None => warning.into(),
        });
      }
      None => {}
    }
    Ok(result)
  }
  #[cfg(not(target_os = "macos"))]
  Err("Focusing the requesting application is currently supported on macOS.".into())
}

#[cfg(target_os = "macos")]
mod macos {
  use super::*;
  use objc2::rc::Retained;
  use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
  use objc2_foundation::NSString;

  fn app_for_pid(mut pid: u32) -> Option<Retained<NSRunningApplication>> {
    for _ in 0..32 {
      if pid <= 1 || pid > i32::MAX as u32 {
        break;
      }
      if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32) {
        if app.bundleIdentifier().is_some() {
          return Some(app);
        }
      }
      let parent = output(Command::new("/bin/ps").args(["-p", &pid.to_string(), "-o", "ppid="]))
        .ok()?
        .parse()
        .ok()?;
      if parent == pid {
        break;
      }
      pid = parent;
    }
    None
  }

  pub(super) fn applications(origin: &Origin) -> Vec<Retained<NSRunningApplication>> {
    if let Some(bundle) = origin.bundle_id() {
      let apps =
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle));
      let all: Vec<_> = apps.iter().collect();
      if let Some(pid) = origin.app_pid {
        if let Some(app) = all.iter().find(|app| app.processIdentifier() == pid as i32) {
          return vec![app.clone()];
        }
      }
      return all;
    }
    for pid in [origin.app_pid, origin.pid].into_iter().flatten() {
      if let Some(app) = app_for_pid(pid) {
        return vec![app];
      }
    }
    // An ID or TTY without an app can still be found among supported terminals.
    if origin.terminal_id.is_some()
      || origin.tty.is_some()
      || origin.window_id.is_some()
      || origin.window_title.is_some()
    {
      return [
        "com.mitchellh.ghostty",
        "com.apple.Terminal",
        "com.googlecode.iterm2",
      ]
      .into_iter()
      .flat_map(|id| {
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(id))
          .to_vec()
      })
      .collect();
    }
    Vec::new()
  }

  pub(super) fn attempt(app: &NSRunningApplication, target: &Target) -> Result<(), String> {
    if matches!(target, Target::Application) {
      app.unhide();
      #[allow(deprecated)]
      return if app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps) {
        Ok(())
      } else {
        Err("macOS could not activate the requesting application.".into())
      };
    }
    let bundle = app
      .bundleIdentifier()
      .map(|id| id.to_string())
      .unwrap_or_default();
    let (mode, value) = match target {
      Target::Terminal(v) => ("terminal", v),
      Target::Tty(v) => ("tty", v),
      Target::WindowId(v) => ("window", v),
      Target::WindowTitle(v) => ("title", v),
      Target::Application => unreachable!(),
    };
    if (bundle == "com.mitchellh.ghostty" && mode == "tty")
      || (bundle == "com.apple.Terminal" && mode == "terminal")
    {
      return Err(
        "This terminal does not expose that session identifier; using a broader target.".into(),
      );
    }
    let script = match bundle.as_str() {
      "com.mitchellh.ghostty" => include_str!("focus/ghostty.applescript"),
      "com.apple.Terminal" => include_str!("focus/terminal.applescript"),
      "com.googlecode.iterm2" => include_str!("focus/iterm.applescript"),
      _ if mode == "title" => include_str!("focus/window.applescript"),
      _ => return Err("This app does not support the supplied terminal/window identifier.".into()),
    };
    // Arguments are data. Notification strings are never inserted into code.
    let result = output(Command::new("/usr/bin/osascript").args([
      "-e",
      script,
      mode,
      value,
      &app.processIdentifier().to_string(),
    ]))?;
    if result == "focused" {
      Ok(())
    } else {
      Err(format!(
        "The requested {mode} was not found or was ambiguous."
      ))
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn falls_back_from_closed_pane_to_window_before_app() {
    let origin = Origin {
      terminal_id: Some("closed".into()),
      window_id: Some("open".into()),
      terminal_app: Some("ghostty".into()),
      ..Default::default()
    };
    let mut seen = Vec::new();
    let result = try_targets(&origin, |target| {
      seen.push(target.clone());
      if matches!(target, Target::WindowId(_)) {
        Ok(())
      } else {
        Err("Pane closed.".into())
      }
    })
    .unwrap();
    assert_eq!(
      seen,
      vec![
        Target::Terminal("closed".into()),
        Target::WindowId("open".into())
      ]
    );
    assert_eq!(result.target, "window");
    assert!(result.warning.unwrap().contains("Pane closed"));
  }

  #[test]
  fn app_only_and_failed_precision_still_focus_app() {
    let origin = Origin {
      terminal_app: Some("ghostty".into()),
      ..Default::default()
    };
    assert_eq!(targets(&origin), [Target::Application]);
    let origin = Origin {
      tty: Some("/dev/ttys001".into()),
      ..origin
    };
    let result = try_targets(&origin, |target| {
      if matches!(target, Target::Application) {
        Ok(())
      } else {
        Err("No TTY match.".into())
      }
    })
    .unwrap();
    assert_eq!(result.target, "application");
  }

  #[test]
  fn total_failure_is_not_reported_as_success() {
    assert!(try_targets(&Origin::default(), |_| Err("App closed.".into())).is_err());
  }
}
