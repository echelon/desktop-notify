#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod model;

use std::sync::{
  atomic::{AtomicBool, Ordering},
  Mutex,
};
use std::time::Duration;

use model::{visibility_change, Notification, Snapshot, VisibilityChange};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State};

const SERVICE: &str = "http://127.0.0.1:43110";
const EVENT: &str = "notification-state";

struct AppState {
  service: String,
  client: reqwest::Client,
  snapshot: Mutex<Snapshot>,
  ready: AtomicBool,
}

fn show_window(app: &AppHandle) {
  if let Some(window) = app.get_webview_window("main") {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
  }
}

fn hide(app: &AppHandle) {
  if let Some(window) = app.get_webview_window("main") {
    let _ = window.hide();
  }
}

#[tauri::command]
fn hide_window(app: AppHandle) {
  hide(&app);
}

#[tauri::command]
fn get_snapshot(state: State<'_, AppState>) -> Snapshot {
  state
    .snapshot
    .lock()
    .unwrap_or_else(|e| e.into_inner())
    .clone()
}

#[tauri::command]
fn window_ready(app: AppHandle, state: State<'_, AppState>) {
  state.ready.store(true, Ordering::SeqCst);
  if state
    .snapshot
    .lock()
    .unwrap_or_else(|e| e.into_inner())
    .notification
    .is_some()
  {
    show_window(&app);
  }
}

#[tauri::command]
async fn dismiss_notification(id: String, state: State<'_, AppState>) -> Result<(), String> {
  // IDs are server-generated hex strings; never allow user text into a URL path.
  if id.len() != 32 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
    return Err("Invalid notification ID".into());
  }
  state
    .client
    .post(format!("{}/dismiss/{id}", state.service))
    .send()
    .await
    .map_err(|_| "Cannot reach the notification service. Please try again.".to_string())?
    .error_for_status()
    .map_err(|e| e.to_string())?;
  // The poller reconciles state and hides only if there is no newer alert.
  Ok(())
}

async fn read_notification(
  client: &reqwest::Client,
  service: &str,
) -> Result<Option<Notification>, reqwest::Error> {
  client
    .get(format!("{service}/notification"))
    .send()
    .await?
    .error_for_status()?
    .json()
    .await
}

fn start_polling(app: AppHandle) {
  tauri::async_runtime::spawn(async move {
    let mut tick = 0u32;
    loop {
      let state = app.state::<AppState>();
      let result = read_notification(&state.client, &state.service).await;
      let (snapshot, change, changed) = {
        let mut previous = state.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        let next = match result {
          Ok(notification) => Snapshot {
            notification,
            connected: true,
            error: None,
          },
          Err(_) => Snapshot {
            notification: previous.notification.clone(),
            connected: false,
            error: Some("Waiting for the notification service…".into()),
          },
        };
        let change = visibility_change(
          previous.notification.as_ref().map(|n| n.id.as_str()),
          next.notification.as_ref().map(|n| n.id.as_str()),
        );
        let changed = *previous != next;
        *previous = next.clone();
        (next, change, changed)
      };
      if changed {
        let _ = app.emit(EVENT, &snapshot);
        if let Some(tray) = app.tray_by_id("main") {
          let tooltip = match snapshot.notification.as_ref() {
            Some(n) => format!("Desktop Notify — {}", n.title),
            None => "Desktop Notify — All caught up".into(),
          };
          let _ = tray.set_tooltip(Some(&tooltip));
          #[cfg(target_os = "macos")]
          let _ = tray.set_title(snapshot.notification.as_ref().map(|_| "•"));
        }
      }
      if state.ready.load(Ordering::SeqCst) {
        match change {
          VisibilityChange::Show => show_window(&app),
          VisibilityChange::Hide => hide(&app),
          VisibilityChange::Keep => {}
        }
      }
      if changed || tick % 10 == 0 {
        let visible = app
          .get_webview_window("main")
          .is_some_and(|w| w.is_visible().unwrap_or(false));
        let report = serde_json::json!({
          "presentation": "tauri", "window_visible": visible, "pid": std::process::id(),
          "displayed_id": snapshot.notification.as_ref().map(|n| &n.id), "error": snapshot.error,
        });
        let _ = state
          .client
          .post(format!("{}/desktop/status", state.service))
          .json(&report)
          .send()
          .await;
      }
      tick = tick.wrapping_add(1);
      tokio::time::sleep(Duration::from_millis(400)).await;
    }
  });
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
  let show = MenuItem::with_id(app, "show", "Show Notifications", true, None::<&str>)?;
  let hide_item = MenuItem::with_id(app, "hide", "Hide to Tray", true, None::<&str>)?;
  let quit = MenuItem::with_id(app, "quit", "Quit Desktop Notify", true, None::<&str>)?;
  let menu = Menu::with_items(
    app,
    &[
      &show,
      &hide_item,
      &PredefinedMenuItem::separator(app)?,
      &quit,
    ],
  )?;
  TrayIconBuilder::with_id("main")
    .icon(tauri::image::Image::from_bytes(include_bytes!(
      "../icons/tray.png"
    ))?)
    .icon_as_template(true)
    .tooltip("Desktop Notify")
    .menu(&menu)
    .show_menu_on_left_click(false)
    .on_menu_event(|app, event| match event.id.as_ref() {
      "show" => show_window(app),
      "hide" => hide(app),
      "quit" => app.exit(0),
      _ => {}
    })
    .on_tray_icon_event(|tray, event| {
      if matches!(
        event,
        TrayIconEvent::Click {
          button: MouseButton::Left,
          button_state: MouseButtonState::Up,
          ..
        }
      ) {
        show_window(tray.app_handle());
      }
    })
    .build(app)?;
  Ok(())
}

fn main() {
  let args: Vec<_> = std::env::args().collect();
  let service = args
    .windows(2)
    .find(|a| a[0] == "--server-url")
    .map(|a| a[1].clone())
    .unwrap_or_else(|| SERVICE.into());
  let url = reqwest::Url::parse(&service).expect("valid notification service URL");
  assert!(
    url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
    "notification service must use local HTTP"
  );
  let client = reqwest::Client::builder()
    .no_proxy()
    .timeout(Duration::from_secs(2))
    .build()
    .expect("create local notification service client");
  tauri::Builder::default()
    .manage(AppState {
      service: service.trim_end_matches('/').into(),
      client,
      snapshot: Mutex::new(Snapshot::default()),
      ready: AtomicBool::new(false),
    })
    .invoke_handler(tauri::generate_handler![
      get_snapshot,
      window_ready,
      hide_window,
      dismiss_notification
    ])
    .setup(|app| {
      #[cfg(target_os = "macos")]
      app.set_activation_policy(tauri::ActivationPolicy::Accessory);
      build_tray(app)?;
      if let Some(window) = app.get_webview_window("main") {
        // Match Todo's always-on-top, all-Spaces behavior. Add full-screen
        // auxiliary behavior so alerts can also appear over full-screen apps.
        #[cfg(target_os = "macos")]
        unsafe {
          use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
          let raw = window.ns_window()? as *mut NSWindow;
          let native = &*raw;
          native.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
              | NSWindowCollectionBehavior::FullScreenAuxiliary,
          );
          native.setLevel(25); // macOS status-window level, above normal floating windows.
        }
        if let Ok(Some(monitor)) = window.current_monitor() {
          let area = monitor.work_area();
          let size = window.outer_size()?;
          let gap = (20.0 * monitor.scale_factor()) as i32;
          let _ = window.set_position(tauri::PhysicalPosition::new(
            area.position.x + area.size.width as i32 - size.width as i32 - gap,
            area.position.y + gap,
          ));
        }
      }
      start_polling(app.handle().clone());
      Ok(())
    })
    .on_window_event(|window, event| match event {
      tauri::WindowEvent::CloseRequested { api, .. } => {
        api.prevent_close();
        let _ = window.hide();
      }
      tauri::WindowEvent::Resized(_) if window.is_minimized().unwrap_or(false) => {
        let _ = window.hide();
      }
      _ => {}
    })
    .build(tauri::generate_context!())
    .expect("build Desktop Notify")
    .run(|app, event| {
      if let tauri::RunEvent::Reopen { .. } = event {
        show_window(app);
      }
    });
}
