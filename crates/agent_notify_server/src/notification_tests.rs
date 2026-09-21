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
    .route("/notification", web::get().to(current_notification))
    .route("/notifications", web::get().to(list_notifications))
    .route("/silence/{id}", web::post().to(silence))
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
    assert!(response.origin.is_none());
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
      .first()
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
  assert!(state.notifications.lock().unwrap().active.is_empty());
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
  assert!(state.notifications.lock().unwrap().active.is_empty());
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

#[actix_web::test]
async fn optional_origins_round_trip_through_both_endpoints_and_polling() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state).configure(routes)).await;
  for endpoint in ["/awaiting_user_input", "/all_tasks_finished"] {
    for origin in [
      "null",
      "{}",
      r#"{"terminal_app":"ghostty"}"#,
      r#"{"pid":123}"#,
      r#"{"window_id":"42"}"#,
      r#"{"terminal_app":"ghostty","app_pid":123,"pid":456,"terminal_id":"abc","window_id":"def","window_title":"literal \"title\"; $(echo nope)","tty":"/dev/ttys001","tmux_socket":"/tmp/tmux-test","tmux_pane":"%3","tmux_client":"/dev/ttys001"}"#,
    ] {
      let request = test::TestRequest::post()
        .uri(endpoint)
        .insert_header(("Content-Type", "application/json"))
        .set_payload(format!(
          r#"{{"title":"Task","message":"Which option?","origin":{origin}}}"#
        ))
        .to_request();
      let created: crate::notifications::Notification =
        test::call_and_read_body_json(&app, request).await;
      let polled: crate::notifications::Notification = test::call_and_read_body_json(
        &app,
        test::TestRequest::get().uri("/notification").to_request(),
      )
      .await;
      assert_eq!(polled, created);
      if origin != "null" {
        assert!(created.origin.is_some());
      }
      assert!(matches!(
        commands.try_recv().unwrap(),
        AudioCommand::PlayLoop(_)
      ));
    }
  }
}

#[actix_web::test]
async fn invalid_origin_cannot_replace_an_active_notification() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  let valid = test::TestRequest::post()
    .uri("/awaiting_user_input")
    .insert_header(("Content-Type", "application/json"))
    .set_payload(r#"{"title":"Keep","message":"Original","origin":{"terminal_app":"ghostty"}}"#)
    .to_request();
  let original: crate::notifications::Notification =
    test::call_and_read_body_json(&app, valid).await;
  commands.try_recv().unwrap();
  for origin in [
    r#"{"pid":0}"#,
    r#"{"window_title":"\n"}"#,
    r#"{"tmux_pane":"%1; kill-server"}"#,
    r#"{"unexpected":"field"}"#,
    r#"{"window_id":false}"#,
  ] {
    let invalid = test::TestRequest::post()
      .uri("/all_tasks_finished")
      .insert_header(("Content-Type", "application/json"))
      .set_payload(format!(
        r#"{{"title":"Invalid","message":"Rejected","origin":{origin}}}"#
      ))
      .to_request();
    assert_eq!(
      test::call_service(&app, invalid).await.status(),
      StatusCode::BAD_REQUEST
    );
    assert_eq!(
      state.notifications.lock().unwrap().active.first().unwrap(),
      &original
    );
    assert!(commands.try_recv().is_err());
  }
}

#[actix_web::test]
async fn sessions_update_silence_and_clear_independently() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  let mut created = Vec::new();
  for (session, kind) in [
    ("agent-a", "awaiting_user_input"),
    ("agent-b", "all_tasks_finished"),
  ] {
    let n: crate::notifications::Notification = test::call_and_read_body_json(
      &app,
      test::TestRequest::post().uri(&format!("/{kind}"))
        .set_json(serde_json::json!({"session_id": session, "title": "Same project", "message": "Update", "origin": {"window_id": session}}))
        .to_request(),
    ).await;
    created.push(n);
  }
  let a = &created[0];
  let b = &created[1];
  let list: Vec<crate::notifications::Notification> = test::call_and_read_body_json(
    &app,
    test::TestRequest::get().uri("/notifications").to_request(),
  )
  .await;
  assert_eq!(list, vec![b.clone(), a.clone()]);
  // The newer completion must not interrupt a pending question.
  assert!(matches!(commands.try_recv().unwrap(), AudioCommand::PlayLoop(s) if s.name == "await"));
  assert!(commands.try_recv().is_err());

  // Clearing the non-audible entry cannot silence the pending question.
  let cleared: serde_json::Value = test::call_and_read_body_json(
    &app,
    test::TestRequest::post()
      .uri(&format!("/dismiss/{}", b.id))
      .to_request(),
  )
  .await;
  assert_eq!(cleared["stopped"], true);
  assert!(commands.try_recv().is_err());
  assert_eq!(state.notifications.lock().unwrap().active, vec![a.clone()]);

  // Silence keeps the row and its focus origin intact.
  let muted: serde_json::Value = test::call_and_read_body_json(
    &app,
    test::TestRequest::post()
      .uri(&format!("/silence/{}", a.id))
      .to_request(),
  )
  .await;
  assert_eq!(muted["stopped"], true);
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::StopAll
  ));
  let row = state.notifications.lock().unwrap().active[0].clone();
  assert!(row.silenced);
  assert_eq!(row.origin, a.origin);

  // Another session and the legacy slot coexist with this silenced row.
  for session in [serde_json::json!("agent-b"), serde_json::Value::Null] {
    let _: crate::notifications::Notification = test::call_and_read_body_json(
      &app,
      test::TestRequest::post()
        .uri("/all_tasks_finished")
        .set_json(
          serde_json::json!({"session_id": session, "title": "Done", "message": "Completed"}),
        )
        .to_request(),
    )
    .await;
    assert!(matches!(commands.try_recv().unwrap(), AudioCommand::PlayLoop(s) if s.name == "done"));
  }
  assert_eq!(state.notifications.lock().unwrap().active.len(), 3);
  let others = state.notifications.lock().unwrap().active[..2].to_vec();

  // Updating A re-arms only A, retaining its origin when hints are omitted.
  let replacement: crate::notifications::Notification = test::call_and_read_body_json(
    &app, test::TestRequest::post().uri("/awaiting_user_input")
      .set_json(serde_json::json!({"session_id": "agent-a", "title": "New question", "message": "Which option?"})).to_request(),
  ).await;
  assert_eq!(replacement.origin, a.origin);
  assert!(!replacement.silenced);
  assert_ne!(replacement.id, a.id);
  assert_eq!(state.notifications.lock().unwrap().active[1..], others);
  assert!(matches!(commands.try_recv().unwrap(), AudioCommand::PlayLoop(s) if s.name == "await"));
  for action in ["dismiss", "silence"] {
    let stale: serde_json::Value = test::call_and_read_body_json(
      &app,
      test::TestRequest::post()
        .uri(&format!("/{action}/{}", a.id))
        .to_request(),
    )
    .await;
    assert_eq!(stale["stopped"], false);
  }
  assert!(commands.try_recv().is_err());
  // Clearing A resumes an outstanding completion instead of silencing it.
  let _: serde_json::Value = test::call_and_read_body_json(
    &app,
    test::TestRequest::post()
      .uri(&format!("/dismiss/{}", replacement.id))
      .to_request(),
  )
  .await;
  assert_eq!(state.notifications.lock().unwrap().active, others);
  assert!(matches!(commands.try_recv().unwrap(), AudioCommand::PlayLoop(s) if s.name == "done"));
  let stop = test::TestRequest::post().uri("/stop").to_request();
  assert_eq!(
    test::call_service(&app, stop).await.status(),
    StatusCode::OK
  );
  assert!(state.notifications.lock().unwrap().active.is_empty());
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::StopAll
  ));
}

#[actix_web::test]
async fn invalid_session_ids_cannot_replace_existing_rows() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  let original: crate::notifications::Notification = test::call_and_read_body_json(
    &app,
    test::TestRequest::post()
      .uri("/all_tasks_finished")
      .set_json(serde_json::json!({"session_id": "keep", "title": "Keep", "message": "Original"}))
      .to_request(),
  )
  .await;
  commands.try_recv().unwrap();
  for session in [
    serde_json::json!(""),
    serde_json::json!("  "),
    serde_json::json!("bad\nid"),
    serde_json::json!("x".repeat(257)),
    serde_json::json!(123),
  ] {
    let response = test::call_service(
      &app,
      test::TestRequest::post()
        .uri("/awaiting_user_input")
        .set_json(serde_json::json!({"session_id": session, "title": "Bad", "message": "Rejected"}))
        .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
  }
  assert_eq!(state.notifications.lock().unwrap().active, vec![original]);
  assert!(commands.try_recv().is_err());
}
