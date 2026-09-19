use crate::{
  audio_player::{AudioCommand, AudioPlayerHandle},
  config::{NotifyConfig, DEFAULT_CONFIG_PATH},
  endpoints::{notification_handlers::*, stop_handler::stop_handler},
  server_state::ServerState,
};
use actix_web::{http::StatusCode, test, web, App};
use std::sync::{Arc, Mutex};

fn state() -> (
  web::Data<ServerState>,
  std::sync::mpsc::Receiver<AudioCommand>,
) {
  let (audio, commands) = AudioPlayerHandle::recording();
  (
    web::Data::new(ServerState {
      config: Arc::new(NotifyConfig::read_from_file(DEFAULT_CONFIG_PATH).unwrap()),
      audio,
      notifications: Arc::new(Mutex::new(Default::default())),
    }),
    commands,
  )
}

fn routes(cfg: &mut web::ServiceConfig) {
  cfg
    .route("/awaiting_user_input", web::post().to(awaiting_user_input))
    .route("/all_tasks_finished", web::post().to(all_tasks_finished))
    .route("/dismiss/{id}", web::post().to(dismiss))
    .route("/stop", web::post().to(stop_handler));
}

#[actix_web::test]
async fn notification_replacement_and_stale_dismissal_are_atomic() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  let mut ids = Vec::new();
  for (endpoint, expected) in [
    ("awaiting_user_input", "await"),
    ("all_tasks_finished", "done"),
  ] {
    let request = test::TestRequest::post()
      .uri(&format!("/{endpoint}"))
      .insert_header(("Content-Type", "application/json"))
      .set_payload(r#"{"title":" Work ✓ ","message":" A question or outcome. "}"#)
      .to_request();
    let response: crate::notifications::Notification =
      test::call_and_read_body_json(&app, request).await;
    assert_eq!(response.title, "Work ✓");
    assert_eq!(response.message, "A question or outcome.");
    assert_eq!(response.kind, endpoint);
    ids.push(response.id);
    match commands.try_recv().unwrap() {
      AudioCommand::PlayLoop(spec) => {
        assert_eq!(spec.name, expected);
        assert!(!spec.pool.is_empty());
      }
      other => panic!("unexpected command: {other:?}"),
    }
  }
  assert_ne!(ids[0], ids[1]);
  let stale = test::TestRequest::post()
    .uri(&format!("/dismiss/{}", ids[0]))
    .to_request();
  assert_eq!(
    test::call_service(&app, stale).await.status(),
    StatusCode::OK
  );
  assert!(commands.try_recv().is_err());
  assert_eq!(
    state
      .notifications
      .lock()
      .unwrap()
      .active
      .as_ref()
      .unwrap()
      .id,
    ids[1]
  );
  let current = test::TestRequest::post()
    .uri(&format!("/dismiss/{}", ids[1]))
    .to_request();
  assert_eq!(
    test::call_service(&app, current).await.status(),
    StatusCode::OK
  );
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::StopAll
  ));
  assert!(state.notifications.lock().unwrap().active.is_none());
  let repeat = test::TestRequest::post()
    .uri(&format!("/dismiss/{}", ids[1]))
    .to_request();
  assert_eq!(
    test::call_service(&app, repeat).await.status(),
    StatusCode::OK
  );
  assert!(commands.try_recv().is_err());
}

#[actix_web::test]
async fn rejects_invalid_requests_without_starting_audio() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state).configure(routes)).await;
  for payload in [
    r#"{}"#.to_string(),
    r#"{"title":" ","message":"message"}"#.into(),
    r#"{"title":"title","message":""}"#.into(),
    "not json".into(),
    format!(r#"{{"title":"{}","message":"message"}}"#, "t".repeat(201)),
    format!(r#"{{"title":"title","message":"{}"}}"#, "m".repeat(4001)),
  ] {
    let request = test::TestRequest::post()
      .uri("/all_tasks_finished")
      .insert_header(("Content-Type", "application/json"))
      .set_payload(payload)
      .to_request();
    assert_eq!(
      test::call_service(&app, request).await.status(),
      StatusCode::BAD_REQUEST
    );
  }
  assert!(commands.try_recv().is_err());
  let get = test::TestRequest::get()
    .uri("/all_tasks_finished")
    .to_request();
  assert_eq!(
    test::call_service(&app, get).await.status(),
    StatusCode::NOT_FOUND
  );
}

#[actix_web::test]
async fn stop_clears_notification_and_all_audio() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  let request = test::TestRequest::post()
    .uri("/awaiting_user_input")
    .insert_header(("Content-Type", "application/json"))
    .set_payload(r#"{"title":"Question","message":"Which option?"}"#)
    .to_request();
  assert_eq!(
    test::call_service(&app, request).await.status(),
    StatusCode::OK
  );
  commands.try_recv().unwrap();
  let stop = test::TestRequest::post().uri("/stop").to_request();
  assert_eq!(
    test::call_service(&app, stop).await.status(),
    StatusCode::OK
  );
  assert!(state.notifications.lock().unwrap().active.is_none());
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::StopAll
  ));
}

#[actix_web::test]
async fn missing_sound_does_not_create_a_silent_alert() {
  let (state, commands) = state();
  let state = web::Data::new(ServerState {
    config: Arc::new(NotifyConfig::default()),
    ..state.get_ref().clone()
  });
  let app = test::init_service(App::new().app_data(state).configure(routes)).await;
  let request = test::TestRequest::post()
    .uri("/all_tasks_finished")
    .insert_header(("Content-Type", "application/json"))
    .set_payload(r#"{"title":"Finished","message":"Tests passed."}"#)
    .to_request();
  assert_eq!(
    test::call_service(&app, request).await.status(),
    StatusCode::SERVICE_UNAVAILABLE
  );
  assert!(commands.try_recv().is_err());
}
