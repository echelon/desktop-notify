use crate::{
  audio_player::{AudioCommand, AudioPlayerHandle},
  config::{NotifyConfig, DEFAULT_CONFIG_PATH},
  endpoints::{notification_handlers::*, stop_handler},
  server_state::ServerState,
};
use actix_web::{http::StatusCode, test, web, App};
use chrono::{TimeDelta, Utc};
use notify_types::{Notification, SoundState, TaskState};
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
    .route("/notifications", web::get().to(list_notifications))
    .route("/working", web::post().to(working))
    .route("/task_failed", web::post().to(task_failed))
    .route("/acknowledge/{id}", web::post().to(acknowledge))
    .route("/focused/{id}", web::post().to(focused))
    .route("/sound", web::get().to(sound))
    .route("/sound/snooze", web::post().to(snooze))
    .route("/sound/resume", web::post().to(resume_sound))
    .configure(stop_handler::routes);
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
    let expected_state = if endpoint == "awaiting_user_input" {
      TaskState::InputNeeded
    } else {
      TaskState::Done
    };
    assert_eq!(response.state, expected_state);
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
      let polled: Vec<crate::notifications::Notification> = test::call_and_read_body_json(
        &app,
        test::TestRequest::get().uri("/notifications").to_request(),
      )
      .await;
      assert_eq!(polled[0], created);
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
      .uri(&format!("/acknowledge/{}", a.id))
      .to_request(),
  )
  .await;
  assert_eq!(muted["stopped"], true);
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::StopAll
  ));
  let row = state.notifications.lock().unwrap().active[0].clone();
  assert_eq!(row.state, TaskState::InputNeededIgnored);
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
  assert!(replacement.state.is_alerting());
  assert_ne!(replacement.id, a.id);
  assert_eq!(state.notifications.lock().unwrap().active[1..], others);
  assert!(matches!(commands.try_recv().unwrap(), AudioCommand::PlayLoop(s) if s.name == "await"));
  for action in ["dismiss", "acknowledge"] {
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

#[actix_web::test]
async fn session_context_round_trips_merges_clears_and_stays_scoped() {
  let (state, _commands) = state();
  let app = test::init_service(App::new().app_data(state).configure(routes)).await;
  let full = serde_json::json!({"cwd": "/workspace/one", "repo_name": "One", "repo_description": "First repo", "work_arc": "Improve reliability", "current_ask": "Fix login"});
  let requests = [
    (Some("a"), "/awaiting_user_input", full.clone()),
    (
      Some("b"),
      "/all_tasks_finished",
      serde_json::json!({"repo_name": "Other"}),
    ),
    (
      Some("a"),
      "/all_tasks_finished",
      serde_json::json!({"current_ask": "Add tests"}),
    ),
    (Some("a"), "/awaiting_user_input", serde_json::Value::Null),
    (
      Some("a"),
      "/all_tasks_finished",
      serde_json::json!({"work_arc": " Ship the release ", "repo_description": ""}),
    ),
    (
      Some("a"),
      "/awaiting_user_input",
      serde_json::json!({"cwd": "/workspace/two"}),
    ),
    (None, "/all_tasks_finished", full),
    (None, "/all_tasks_finished", serde_json::Value::Null),
  ];
  let mut responses = Vec::new();
  for (session, endpoint, context) in requests {
    let created: serde_json::Value = test::call_and_read_body_json(
      &app, test::TestRequest::post().uri(endpoint)
        .set_json(serde_json::json!({"session_id": session, "title": "Update", "message": "Progress", "context": context})).to_request(),
    ).await;
    let polled: serde_json::Value = test::call_and_read_body_json(
      &app,
      test::TestRequest::get().uri("/notifications").to_request(),
    )
    .await;
    assert_eq!(polled[0], created);
    if responses.len() >= 2 {
      let other = polled
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["session_id"] == "b")
        .unwrap();
      assert_eq!(other, &responses[1]);
    }
    responses.push(created);
  }
  assert_eq!(responses[2]["context"]["repo_name"], "One");
  assert_eq!(responses[2]["context"]["current_ask"], "Add tests");
  assert_eq!(responses[2]["context"], responses[3]["context"]);
  assert_eq!(responses[4]["context"]["work_arc"], "Ship the release");
  assert!(responses[4]["context"].get("repo_description").is_none());
  assert!(responses[5]["context"].get("repo_name").is_none());
  assert_eq!(responses[5]["context"]["current_ask"], "Add tests");
  // Missing session IDs don't let an unrelated caller inherit another's context.
  assert!(responses[7].get("context").is_none());
}

#[actix_web::test]
async fn invalid_context_cannot_replace_an_active_entry_or_its_sound() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  let original: crate::notifications::Notification = test::call_and_read_body_json(
    &app,
    test::TestRequest::post()
      .uri("/awaiting_user_input")
      .set_json(
        serde_json::json!({"session_id": "keep", "title": "Original", "message": "Waiting"}),
      )
      .to_request(),
  )
  .await;
  commands.try_recv().unwrap();
  for context in [
    serde_json::json!({"repo_name": "r".repeat(201)}),
    serde_json::json!({"repo_description": "d".repeat(1001)}),
    serde_json::json!({"cwd": "p".repeat(4097)}),
    serde_json::json!({"work_arc": "a".repeat(2001)}),
    serde_json::json!({"current_ask": "a".repeat(2001)}),
    serde_json::json!({"current_ask": "bad\u{0}data"}),
    serde_json::json!({"repo_name": false}),
    serde_json::json!({"unknown": "bad"}),
  ] {
    let response = test::call_service(
      &app, test::TestRequest::post().uri("/all_tasks_finished")
        .set_json(serde_json::json!({"session_id": "keep", "title": "Rejected", "message": "Invalid context", "context": context})).to_request(),
    ).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
  }
  assert_eq!(state.notifications.lock().unwrap().active, vec![original]);
  assert!(commands.try_recv().is_err());
}

fn alert_request(endpoint: &str, session: &str) -> test::TestRequest {
  test::TestRequest::post()
    .uri(&format!("/{endpoint}"))
    .insert_header(("Content-Type", "application/json"))
    .set_payload(format!(
      r#"{{"session_id":"{session}","title":"Task","message":"Update"}}"#
    ))
}

fn snooze_request(seconds: i64) -> test::TestRequest {
  test::TestRequest::post()
    .uri("/sound/snooze")
    .insert_header(("Content-Type", "application/json"))
    .set_payload(format!(r#"{{"seconds":{seconds}}}"#))
}

#[actix_web::test]
async fn legacy_stop_sound_preserves_rows_focus_and_waiting_tools() {
  for path in ["/stop", "/sound/stop"] {
    let (state, commands) = state();
    let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
    for (endpoint, session) in [
      ("awaiting_user_input", "question"),
      ("all_tasks_finished", "done"),
      ("task_failed", "failed"),
      ("working", "busy"),
    ] {
      test::call_service(&app, test::TestRequest::post().uri(&format!("/{endpoint}"))
      .set_json(serde_json::json!({
        "session_id": session, "title": "Keep this task", "message": "Keep this message",
        "tool_use_id": "pending-tool", "agent": "codex",
        "context": {"repo_name": "same-repo"},
        "origin": {"terminal_app": "com.mitchellh.ghostty", "terminal_id": session, "tmux_pane": "%5"}
      })).to_request()).await;
    }
    test::call_service(&app, snooze_request(300).to_request()).await;
    let (mut expected, waiting) = {
      let current = state.notifications.lock().unwrap();
      (current.active.clone(), current.waiting_tools.clone())
    };
    assert_eq!(expected.len(), 4);
    assert_eq!(
      waiting.get("question").map(String::as_str),
      Some("pending-tool")
    );
    for row in &mut expected {
      row.state = match row.state {
        TaskState::InputNeeded => TaskState::InputNeededIgnored,
        TaskState::Done => TaskState::DoneAcknowledged,
        TaskState::Failed => TaskState::FailedAcknowledged,
        other => other,
      };
    }
    while commands.try_recv().is_ok() {}
    let response: SoundState =
      test::call_and_read_body_json(&app, test::TestRequest::get().uri(path).to_request()).await;
    assert_eq!(response, SoundState::default());
    let rows: Vec<Notification> = test::call_and_read_body_json(
      &app,
      test::TestRequest::get().uri("/notifications").to_request(),
    )
    .await;
    for (row, expected) in rows.iter().zip(&mut expected) {
      // Stopping sound records the dismissal on each row it quieted.
      assert_eq!(
        row.times.dismissed_at.is_some(),
        expected.state != TaskState::Working
      );
      assert_eq!(row.times.user_action_at, row.times.dismissed_at);
      expected.times.dismissed_at = row.times.dismissed_at;
      expected.times.user_action_at = row.times.user_action_at;
      if row.times.dismissed_at.is_some() {
        expected.times.last_request_at = row.times.dismissed_at;
      }
    }
    assert_eq!(rows, expected); // IDs, order, text, context and Focus targets survive.
    assert_eq!(state.notifications.lock().unwrap().waiting_tools, waiting);
    assert!(commands
      .try_iter()
      .all(|command| matches!(command, AudioCommand::StopAll)));

    // Stopping sound is permanent for these updates; a fresh update alerts again.
    test::call_service(
      &app,
      alert_request("all_tasks_finished", "done").to_request(),
    )
    .await;
    assert!(matches!(
      commands.try_recv().unwrap(),
      AudioCommand::PlayLoop(_)
    ));
  }
}

#[actix_web::test]
async fn global_stop_sound_silences_every_row_until_a_fresh_update() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  test::call_service(&app, alert_request("awaiting_user_input", "a").to_request()).await;
  test::call_service(&app, alert_request("all_tasks_finished", "b").to_request()).await;
  while commands.try_recv().is_ok() {}

  let silence = test::TestRequest::post().uri("/sound/stop").to_request();
  let response: SoundState = test::call_and_read_body_json(&app, silence).await;
  assert_eq!(response, SoundState::default());
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::StopAll
  ));
  assert!(commands.try_recv().is_err());
  {
    let current = state.notifications.lock().unwrap();
    assert_eq!(current.active.len(), 2);
    assert!(current.active.iter().all(|n| !n.state.is_alerting()));
  }

  // A later update is a new alert and joins the shared loop again.
  let update = alert_request("all_tasks_finished", "a").to_request();
  let fresh: Notification = test::call_and_read_body_json(&app, update).await;
  assert!(fresh.state.is_alerting());
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::PlayLoop(spec) if spec.name == "done"
  ));
  let sound = test::TestRequest::get().uri("/sound").to_request();
  let sound: SoundState = test::call_and_read_body_json(&app, sound).await;
  assert!(sound.alerting);
}

#[actix_web::test]
async fn snooze_mutes_new_alerts_and_resumes_after_its_wall_clock_deadline() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  test::call_service(&app, alert_request("awaiting_user_input", "a").to_request()).await;
  commands.try_recv().unwrap();

  let too_long = notify_types::MAX_SNOOZE.num_seconds() + 1;
  for seconds in [0, -5, too_long] {
    let invalid = test::call_service(&app, snooze_request(seconds).to_request()).await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
  }
  assert!(commands.try_recv().is_err());

  let requested_at = Utc::now();
  let snoozed: SoundState =
    test::call_and_read_body_json(&app, snooze_request(300).to_request()).await;
  let until = snoozed.snoozed_until.unwrap();
  let expected = requested_at + TimeDelta::minutes(5);
  assert!(until >= expected && until - expected < TimeDelta::seconds(10));
  assert!(snoozed.alerting);
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::StopAll
  ));

  // Alerts during the snooze are recorded as alerting and stay quiet; polling
  // before the deadline leaves the loop stopped.
  let update = alert_request("awaiting_user_input", "b").to_request();
  let question: Notification = test::call_and_read_body_json(&app, update).await;
  assert!(question.state.is_alerting());
  let poll = test::TestRequest::get().uri("/notifications").to_request();
  assert_eq!(
    test::call_service(&app, poll).await.status(),
    StatusCode::OK
  );
  assert!(commands.try_recv().is_err());

  // Move the recorded deadline into the past; the next poll notices it.
  state.notifications.lock().unwrap().snoozed_until = Some(Utc::now() - TimeDelta::seconds(1));
  let poll = test::TestRequest::get().uri("/notifications").to_request();
  assert_eq!(
    test::call_service(&app, poll).await.status(),
    StatusCode::OK
  );
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::PlayLoop(spec) if spec.name == "await"
  ));
  {
    let current = state.notifications.lock().unwrap();
    assert_eq!(current.snoozed_until, None);
    assert_eq!(current.audio_id.as_deref(), Some(question.id.as_str()));
  }
  let poll = test::TestRequest::get().uri("/notifications").to_request();
  test::call_service(&app, poll).await;
  assert!(commands.try_recv().is_err());
}

#[actix_web::test]
async fn resume_stop_and_clock_jumps_end_a_snooze() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  test::call_service(&app, alert_request("all_tasks_finished", "a").to_request()).await;
  test::call_service(&app, snooze_request(60).to_request()).await;
  while commands.try_recv().is_ok() {}

  let resume = test::TestRequest::post().uri("/sound/resume").to_request();
  let resumed: SoundState = test::call_and_read_body_json(&app, resume).await;
  assert_eq!(resumed.snoozed_until, None);
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::PlayLoop(_)
  ));

  // A deadline beyond the maximum snooze means the wall clock went backwards.
  test::call_service(&app, snooze_request(60).to_request()).await;
  commands.try_recv().unwrap();
  state.notifications.lock().unwrap().snoozed_until = Some(Utc::now() + TimeDelta::days(3));
  let sound = test::TestRequest::get().uri("/sound").to_request();
  let sound: SoundState = test::call_and_read_body_json(&app, sound).await;
  assert_eq!(sound.snoozed_until, None);
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::PlayLoop(_)
  ));

  test::call_service(&app, snooze_request(60).to_request()).await;
  test::call_service(&app, test::TestRequest::post().uri("/stop").to_request()).await;
  assert_eq!(state.notifications.lock().unwrap().snoozed_until, None);
}

fn working_request(body: &str) -> test::TestRequest {
  test::TestRequest::post()
    .uri("/working")
    .insert_header(("Content-Type", "application/json"))
    .set_payload(body.to_owned())
}

fn row_state(state: &web::Data<ServerState>, session: &str) -> Option<TaskState> {
  let current = state.notifications.lock().unwrap();
  current
    .active
    .iter()
    .find(|n| n.session_id.as_deref() == Some(session))
    .map(|n| n.state)
}

#[actix_web::test]
async fn dismissing_one_task_leaves_the_others_alerting() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  let done: Notification =
    test::call_and_read_body_json(&app, alert_request("all_tasks_finished", "a").to_request())
      .await;
  let question: Notification =
    test::call_and_read_body_json(&app, alert_request("awaiting_user_input", "b").to_request())
      .await;
  let failure: Notification =
    test::call_and_read_body_json(&app, alert_request("task_failed", "c").to_request()).await;
  assert_eq!(done.state, TaskState::Done);
  assert_eq!(failure.state, TaskState::Failed);
  while commands.try_recv().is_ok() {}

  // Acknowledging the audible question falls back to the failure's await loop.
  let ack = test::TestRequest::post()
    .uri(&format!("/acknowledge/{}", question.id))
    .to_request();
  let response: serde_json::Value = test::call_and_read_body_json(&app, ack).await;
  assert_eq!(response["stopped"], true);
  assert_eq!(row_state(&state, "b"), Some(TaskState::InputNeededIgnored));
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::PlayLoop(spec) if spec.name == "await"
  ));
  let ack = test::TestRequest::post()
    .uri(&format!("/acknowledge/{}", failure.id))
    .to_request();
  test::call_service(&app, ack).await;
  assert_eq!(row_state(&state, "c"), Some(TaskState::FailedAcknowledged));
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::PlayLoop(spec) if spec.name == "done"
  ));
  assert_eq!(row_state(&state, "a"), Some(TaskState::Done));

  // Already-quiet and stale IDs are harmless; rows stay listed.
  for id in [&question.id, &"0".repeat(32)] {
    let ack = test::TestRequest::post()
      .uri(&format!("/acknowledge/{id}"))
      .to_request();
    let response: serde_json::Value = test::call_and_read_body_json(&app, ack).await;
    assert_eq!(response["stopped"], false);
  }
  assert!(commands.try_recv().is_err());
  assert_eq!(state.notifications.lock().unwrap().active.len(), 3);

  // Acknowledging the last alerting row stops the loop.
  let ack = test::TestRequest::post()
    .uri(&format!("/acknowledge/{}", done.id))
    .to_request();
  test::call_service(&app, ack).await;
  assert_eq!(row_state(&state, "a"), Some(TaskState::DoneAcknowledged));
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::StopAll
  ));
}

#[actix_web::test]
async fn working_stops_a_sessions_alert_and_only_resumes_waiting_rows_when_asked() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;

  // A tool finishing in a session with no waiting row changes nothing.
  let noop: serde_json::Value = test::call_and_read_body_json(
    &app,
    working_request(r#"{"session_id":"a","only_if_waiting":true}"#).to_request(),
  )
  .await;
  assert_eq!(noop, serde_json::json!({"updated": false}));
  assert!(state.notifications.lock().unwrap().active.is_empty());

  // A submitted prompt creates a quiet working row.
  let started: serde_json::Value = test::call_and_read_body_json(
    &app,
    working_request(r#"{"session_id":"a","title":"repo: Fix login","message":"Fix login"}"#)
      .to_request(),
  )
  .await;
  assert_eq!(started["notification"]["state"], "working");
  assert_eq!(row_state(&state, "a"), Some(TaskState::Working));
  assert!(commands.try_recv().is_err());

  let question: Notification =
    test::call_and_read_body_json(&app, alert_request("awaiting_user_input", "a").to_request())
      .await;
  commands.try_recv().unwrap();
  test::call_service(&app, alert_request("all_tasks_finished", "b").to_request()).await;
  // The question keeps priority, so the completion does not restart the loop.
  assert!(commands.try_recv().is_err());

  // Answering the question resumes work: fresh ID, text kept, sound moves on.
  let resumed: serde_json::Value = test::call_and_read_body_json(
    &app,
    working_request(r#"{"session_id":"a","only_if_waiting":true}"#).to_request(),
  )
  .await;
  assert_eq!(resumed["updated"], true);
  assert_ne!(resumed["notification"]["id"], question.id.as_str());
  assert_eq!(resumed["notification"]["title"], "Task");
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::PlayLoop(spec) if spec.name == "done"
  ));

  // A finished row is not revived by a per-tool hook, only by a new prompt.
  let noop: serde_json::Value = test::call_and_read_body_json(
    &app,
    working_request(r#"{"session_id":"b","only_if_waiting":true}"#).to_request(),
  )
  .await;
  assert_eq!(noop["updated"], false);
  assert_eq!(row_state(&state, "b"), Some(TaskState::Done));
  test::call_service(&app, working_request(r#"{"session_id":"b"}"#).to_request()).await;
  assert_eq!(row_state(&state, "b"), Some(TaskState::Working));
  assert!(matches!(
    commands.try_recv().unwrap(),
    AudioCommand::StopAll
  ));

  for bad in [
    r#"{"only_if_waiting":true}"#,
    r#"{"session_id":" "}"#,
    r#"{"session_id":"a","extra":1}"#,
  ] {
    let response = test::call_service(&app, working_request(bad).to_request()).await;
    assert!(response.status().is_client_error(), "{bad}");
  }
}

#[actix_web::test]
async fn a_parallel_tool_finishing_does_not_resume_a_row_waiting_on_another_call() {
  let (state, _commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  let ask = test::TestRequest::post()
    .uri("/awaiting_user_input")
    .insert_header(("Content-Type", "application/json"))
    .set_payload(
      r#"{"session_id":"a","title":"Allow?","message":"rm -rf build","tool_use_id":"tool-A"}"#,
    )
    .to_request();
  test::call_service(&app, ask).await;
  let finished = |tool: &str| {
    working_request(&format!(
      r#"{{"session_id":"a","only_if_waiting":true,"tool_use_id":"{tool}"}}"#
    ))
    .to_request()
  };
  let other: serde_json::Value = test::call_and_read_body_json(&app, finished("tool-B")).await;
  assert_eq!(other["updated"], false);
  assert_eq!(row_state(&state, "a"), Some(TaskState::InputNeeded));
  let same: serde_json::Value = test::call_and_read_body_json(&app, finished("tool-A")).await;
  assert_eq!(same["updated"], true);
  assert_eq!(row_state(&state, "a"), Some(TaskState::Working));
  assert!(state.notifications.lock().unwrap().waiting_tools.is_empty());
  let bad = working_request(r#"{"session_id":"a","tool_use_id":"x\u0007"}"#).to_request();
  assert_eq!(
    test::call_service(&app, bad).await.status(),
    StatusCode::BAD_REQUEST
  );
}

#[actix_web::test]
async fn the_reporting_agent_is_optional_and_kept_across_updates() {
  let (state, _commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  let post = |body: &str| {
    test::TestRequest::post()
      .uri("/awaiting_user_input")
      .insert_header(("Content-Type", "application/json"))
      .set_payload(body.to_owned())
      .to_request()
  };
  let first: Notification = test::call_and_read_body_json(
    &app,
    post(r#"{"session_id":"a","title":"T","message":"M","agent":"claude_code"}"#),
  )
  .await;
  assert_eq!(first.agent, Some(notify_types::Agent::ClaudeCode));
  // A per-tool resume sends no agent; the row keeps it.
  let resumed: serde_json::Value = test::call_and_read_body_json(
    &app,
    working_request(r#"{"session_id":"a","only_if_waiting":true}"#).to_request(),
  )
  .await;
  assert_eq!(resumed["notification"]["agent"], "claude_code");
  let unassigned: Notification =
    test::call_and_read_body_json(&app, post(r#"{"title":"T","message":"M"}"#)).await;
  assert_eq!(unassigned.agent, None);
  assert!(!serde_json::to_string(&unassigned)
    .unwrap()
    .contains("agent"));
}

fn times(state: &web::Data<ServerState>, session: &str) -> notify_types::TaskTimes {
  let current = state.notifications.lock().unwrap();
  current
    .active
    .iter()
    .find(|n| n.session_id.as_deref() == Some(session))
    .map(|n| n.times.clone())
    .unwrap()
}

#[actix_web::test]
async fn timestamps_follow_a_task_through_waiting_finishing_and_dismissal() {
  let (state, _commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  let before = Utc::now();
  test::call_service(
    &app,
    working_request(r#"{"session_id":"a","title":"Build","message":"Build it"}"#).to_request(),
  )
  .await;
  let first = times(&state, "a");
  let started = first.task_started_at.unwrap();
  assert!(started >= before);
  assert_eq!(first.tracked_since, Some(started));
  assert_eq!(first.updated_at, Some(started));
  // Submitting the prompt was the user's input; nothing came from the app yet.
  assert_eq!(first.user_input_at, Some(started));
  assert_eq!(first.last_request_at, Some(started));
  assert_eq!(first.user_action_at, None);
  assert_eq!(
    (
      first.task_finished_at,
      first.waiting_since,
      first.dismissed_at
    ),
    (None, None, None)
  );

  // A prompt queued mid-turn joins the running task.
  test::call_service(
    &app,
    working_request(r#"{"session_id":"a","title":"Also","message":"And this"}"#).to_request(),
  )
  .await;
  assert_eq!(times(&state, "a").task_started_at, Some(started));

  // Waiting keeps the task's start and records when waiting began; dismissing
  // the question records that without counting as an agent update.
  let question: Notification =
    test::call_and_read_body_json(&app, alert_request("awaiting_user_input", "a").to_request())
      .await;
  let waiting = question.times.waiting_since.unwrap();
  assert_eq!(question.times.task_started_at, Some(started));
  assert_eq!(question.times.task_finished_at, None);
  test::call_service(
    &app,
    test::TestRequest::post()
      .uri(&format!("/acknowledge/{}", question.id))
      .to_request(),
  )
  .await;
  let ignored = times(&state, "a");
  assert!(ignored.dismissed_at.unwrap() >= waiting);
  assert_eq!(ignored.updated_at, question.times.updated_at);
  assert_eq!(ignored.user_action_at, ignored.dismissed_at);
  assert_eq!(ignored.last_request_at, ignored.dismissed_at);
  assert_eq!(ignored.user_input_at, question.times.user_input_at);
  assert_eq!(ignored.waiting_since, Some(waiting));

  // Resuming continues the task and clears the waiting/dismissal marks.
  test::call_service(
    &app,
    working_request(r#"{"session_id":"a","only_if_waiting":true}"#).to_request(),
  )
  .await;
  let resumed = times(&state, "a");
  assert_eq!(resumed.task_started_at, Some(started));
  assert_eq!((resumed.waiting_since, resumed.dismissed_at), (None, None));
  // Answering the question is terminal input; the earlier dismissal is kept.
  assert_eq!(resumed.user_input_at, resumed.updated_at);
  assert_eq!(resumed.user_action_at, ignored.user_action_at);

  // A finished tool that changes nothing still counts as news from the agent.
  test::call_service(
    &app,
    working_request(r#"{"session_id":"a","only_if_waiting":true}"#).to_request(),
  )
  .await;
  let tool = times(&state, "a");
  assert!(tool.updated_at >= resumed.updated_at);
  assert_eq!(tool.last_request_at, tool.updated_at);
  assert_eq!(tool.task_started_at, Some(started));
  // A finished tool is the agent's own progress, not user input.
  assert_eq!(tool.user_input_at, resumed.user_input_at);

  let done: Notification =
    test::call_and_read_body_json(&app, alert_request("all_tasks_finished", "a").to_request())
      .await;
  assert_eq!(done.times.task_started_at, Some(started));
  assert_eq!(done.times.task_finished_at, done.times.updated_at);
  assert_eq!(done.times.tracked_since, Some(started));
  test::call_service(
    &app,
    test::TestRequest::post().uri("/sound/stop").to_request(),
  )
  .await;
  assert!(times(&state, "a").dismissed_at.is_some());

  // The next prompt starts a new task; only the first-seen time endures.
  test::call_service(
    &app,
    working_request(r#"{"session_id":"a","title":"Next","message":"Next task"}"#).to_request(),
  )
  .await;
  let next = times(&state, "a");
  assert!(next.task_started_at.unwrap() >= done.times.task_finished_at.unwrap());
  assert_eq!(next.tracked_since, Some(started));
  assert_eq!((next.task_finished_at, next.dismissed_at), (None, None));
}

#[actix_web::test]
async fn unobserved_task_starts_stay_unknown_and_sessions_keep_their_own_times() {
  let (state, _commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  let failed: Notification =
    test::call_and_read_body_json(&app, alert_request("task_failed", "a").to_request()).await;
  assert_eq!(failed.times.task_started_at, None);
  assert!(failed.times.task_finished_at.is_some());
  test::call_service(
    &app,
    working_request(r#"{"session_id":"b","title":"Other","message":"Other task"}"#).to_request(),
  )
  .await;
  assert_eq!(times(&state, "a"), failed.times);
  // A later alert for a finished task does not inherit the finished task's start.
  test::call_service(&app, alert_request("all_tasks_finished", "b").to_request()).await;
  let finished = times(&state, "b");
  test::call_service(&app, alert_request("all_tasks_finished", "b").to_request()).await;
  assert_eq!(times(&state, "b").task_started_at, None);
  assert_eq!(times(&state, "b").tracked_since, finished.tracked_since);
  // Clearing a row forgets its history.
  let row = state.notifications.lock().unwrap().active[0].id.clone();
  test::call_service(
    &app,
    test::TestRequest::post()
      .uri(&format!("/dismiss/{row}"))
      .to_request(),
  )
  .await;
  test::call_service(&app, alert_request("all_tasks_finished", "b").to_request()).await;
  assert!(times(&state, "b").tracked_since > finished.tracked_since);
}

#[actix_web::test]
async fn focusing_records_a_user_action_without_changing_the_row() {
  let (state, commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  let done: Notification =
    test::call_and_read_body_json(&app, alert_request("all_tasks_finished", "a").to_request())
      .await;
  let other: Notification =
    test::call_and_read_body_json(&app, alert_request("all_tasks_finished", "b").to_request())
      .await;
  while commands.try_recv().is_ok() {}
  let focus = |id: &str| {
    test::TestRequest::post()
      .uri(&format!("/focused/{id}"))
      .to_request()
  };
  let response: serde_json::Value = test::call_and_read_body_json(&app, focus(&done.id)).await;
  assert_eq!(response, serde_json::json!({"recorded": true}));
  let focused = state.notifications.lock().unwrap().active[1].clone();
  let action = focused.times.user_action_at.unwrap();
  assert_eq!(focused.times.last_request_at, Some(action));
  // Only the timestamps moved: same ID, state, order, and sound.
  let mut expected = done.clone();
  expected.times.user_action_at = Some(action);
  expected.times.last_request_at = Some(action);
  assert_eq!(focused, expected);
  assert_eq!(state.notifications.lock().unwrap().active[0], other);
  assert!(commands.try_recv().is_err());
  // Stale IDs are harmless, and later agent updates keep the last action.
  let response: serde_json::Value = test::call_and_read_body_json(&app, focus("stale")).await;
  assert_eq!(response, serde_json::json!({"recorded": false}));
  test::call_service(&app, alert_request("awaiting_user_input", "a").to_request()).await;
  assert_eq!(times(&state, "a").user_action_at, Some(action));
  assert!(times(&state, "a").last_request_at > Some(action));
}

fn alert_with_start(
  endpoint: &str,
  session: &str,
  start: chrono::DateTime<Utc>,
) -> test::TestRequest {
  test::TestRequest::post()
    .uri(&format!("/{endpoint}"))
    .set_json(serde_json::json!({
      "session_id": session, "title": "Task", "message": "Update",
      "turn_started_at": start.to_rfc3339(),
    }))
}

#[actix_web::test]
async fn a_reported_turn_start_only_fills_an_unknown_start_and_never_reaches_back() {
  let (state, _commands) = state();
  let app = test::init_service(App::new().app_data(state.clone()).configure(routes)).await;
  // After a restart the service did not see the prompt: the agent's report fills it.
  let reported = Utc::now() - TimeDelta::minutes(20);
  let done: Notification = test::call_and_read_body_json(
    &app,
    alert_with_start("all_tasks_finished", "a", reported).to_request(),
  )
  .await;
  assert_eq!(done.times.task_started_at, Some(reported));
  let finished = done.times.task_finished_at.unwrap();

  // A later turn reporting a start from before the previous task ended (stale
  // transcript data) must not stretch back over it.
  let failed: Notification = test::call_and_read_body_json(
    &app,
    alert_with_start("task_failed", "a", reported).to_request(),
  )
  .await;
  assert_eq!(failed.times.task_started_at, None);
  // A start after the previous end is a real new turn (e.g. a background task).
  // It can be marginally ahead of the service clock, which clamps it to now.
  let previous_end = failed.times.task_finished_at.unwrap();
  let later = previous_end + TimeDelta::microseconds(1);
  let next: Notification = test::call_and_read_body_json(
    &app,
    alert_with_start("all_tasks_finished", "a", later).to_request(),
  )
  .await;
  let start = next.times.task_started_at.unwrap();
  assert!(start >= previous_end && start <= later);
  assert!(finished < previous_end);

  // An observed start wins over a report, and a far-future report is ignored.
  test::call_service(
    &app,
    working_request(r#"{"session_id":"b","title":"Go","message":"Go"}"#).to_request(),
  )
  .await;
  let observed = times(&state, "b").task_started_at;
  let question: Notification = test::call_and_read_body_json(
    &app,
    alert_with_start("awaiting_user_input", "b", reported).to_request(),
  )
  .await;
  assert_eq!(question.times.task_started_at, observed);
  let future: Notification = test::call_and_read_body_json(
    &app,
    alert_with_start("all_tasks_finished", "c", Utc::now() + TimeDelta::hours(1)).to_request(),
  )
  .await;
  assert_eq!(future.times.task_started_at, None);
}
