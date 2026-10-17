//! The target agent's switch and state, against an owner that answers on
//! the control socket.
#![cfg(feature = "restricted-control-surface")]

use super::*;

fn request(cookie: &str, csrf: Option<&str>, method: Method, body: Value) -> Request<Body> {
    let mut request = json_request(method, "/api/target-agent", body);
    request
        .headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    if let Some(csrf) = csrf {
        request
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
    }
    request
}

async fn call(
    app: &Router,
    cookie: &str,
    csrf: Option<&str>,
    method: Method,
    body: Value,
) -> Value {
    json_body(
        app.clone()
            .oneshot(request(cookie, csrf, method, body))
            .await
            .unwrap(),
    )
    .await
}

/// A request refused before it reaches a handler answers with a status.
async fn refused(app: &Router, cookie: &str, csrf: Option<&str>, body: Value) -> bool {
    let response = app
        .clone()
        .oneshot(request(cookie, csrf, Method::POST, body))
        .await
        .unwrap();
    !response.status().is_success()
}

#[tokio::test]
async fn the_switch_is_the_owners_and_its_state_comes_back() {
    use bli_kvm_control_protocol::ControlResponse;
    use bli_kvm_core::PlatformControlRequestV2 as Request;
    let root = tempdir().unwrap();
    let control = root.path().join("control.sock");
    let mut on = false;
    let seen = super::quic_transport::owner(&control, move |request| match request {
        Request::TargetAgent { enabled } => {
            on = *enabled;
            ControlResponse::Applied
        }
        Request::TargetAgentState {} => ControlResponse::TargetAgentState {
            enabled: on,
            wanted: on,
            open: on,
            problem: None,
            agent: on.then(|| {
                bli_kvm_agent_protocol::Hello::new("hardened-blikvm-agent", "0.1.0", "3c940a7e681c")
            }),
            refused: None,
            messages_in: 2,
            messages_out: 2,
            dropped: 0,
            board_commit: "8dfb152d0c4b1e2f3a4b5c6d7e8f901234567890".into(),
        },
        _ => panic!("unexpected request {request:?}"),
    });
    let app = app_with_options(ServerOptions {
        state_dir: root.path().join("account"),
        control_socket: Some(control),
        restricted_controls_enabled: true,
        ..ServerOptions::default()
    });
    let (cookie, csrf) = setup_and_login(&app).await;

    let manifest = json_body(
        app.clone()
            .oneshot({
                let mut request = json_request(Method::GET, "/api/capabilities", Value::Null);
                request
                    .headers_mut()
                    .insert(header::COOKIE, cookie.parse().unwrap());
                request
            })
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        manifest["data"]["board"]["features"]["target_agent"]["availability"],
        "read_write"
    );

    let state = call(&app, &cookie, None, Method::GET, Value::Null).await;
    assert_eq!(state["data"]["available"], true);
    assert_eq!(state["data"]["enabled"], false);
    assert!(state["data"]["agent"].is_null());

    // Without the CSRF token nothing reaches the owner.
    assert!(refused(&app, &cookie, None, serde_json::json!({"enabled": true})).await);

    let switched = call(
        &app,
        &cookie,
        Some(&csrf),
        Method::POST,
        serde_json::json!({"enabled": true}),
    )
    .await;
    assert_eq!(switched["code"], 0, "{switched}");
    assert_eq!(switched["data"]["enabled"], true);
    assert_eq!(switched["data"]["open"], true);
    assert_eq!(
        switched["data"]["agent"]["program"],
        "hardened-blikvm-agent"
    );
    assert_eq!(switched["data"]["messagesIn"], 2);
    // The board's commit, beside the agent's, for the page to compare.
    assert_eq!(
        switched["data"]["boardCommit"],
        "8dfb152d0c4b1e2f3a4b5c6d7e8f901234567890"
    );

    // A body that is not the switch is refused before the owner.
    assert!(
        refused(
            &app,
            &cookie,
            Some(&csrf),
            serde_json::json!({"enabled": false, "call": "ping"})
        )
        .await
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            Request::TargetAgentState {},
            Request::TargetAgent { enabled: true },
            Request::TargetAgentState {},
        ]
    );
}

#[tokio::test]
async fn a_ping_goes_to_the_agent_and_a_read_needs_the_sessions_lease() {
    use bli_kvm_control_protocol::ControlResponse;
    use bli_kvm_core::PlatformControlRequestV2 as Request;
    let root = tempdir().unwrap();
    let control = root.path().join("control.sock");
    let seen = super::quic_transport::owner(&control, |request| match request {
        Request::TargetAgentCall { .. } => ControlResponse::TargetAgentAnswer {
            answer: bli_kvm_agent_protocol::Answer::Pong,
            round_trip_us: 2_500,
        },
        _ => panic!("unexpected request {request:?}"),
    });
    let app = app_with_options(ServerOptions {
        state_dir: root.path().join("account"),
        control_socket: Some(control),
        restricted_controls_enabled: true,
        ..ServerOptions::default()
    });
    let (cookie, csrf) = setup_and_login(&app).await;
    let call = |body: Value| {
        let mut request = json_request(Method::POST, "/api/target-agent/call", body);
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        request
    };
    let answered = json_body(
        app.clone()
            .oneshot(call(serde_json::json!({"call": "ping"})))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(answered["code"], 0, "{answered}");
    assert_eq!(answered["data"]["answer"], "pong");
    assert_eq!(answered["data"]["roundTripUs"], 2500);
    // Without the keyboard's and mouse's lease the read never reaches the
    // owner; an unknown call is refused as a body.
    let refused = json_body(
        app.clone()
            .oneshot(call(serde_json::json!({"call": "monitors"})))
            .await
            .unwrap(),
    )
    .await;
    assert_ne!(refused["code"], 0);
    let unknown = app
        .clone()
        .oneshot(call(serde_json::json!({"call": "format_disk"})))
        .await
        .unwrap();
    assert!(!unknown.status().is_success());
    assert_eq!(
        *seen.lock().unwrap(),
        vec![Request::TargetAgentCall {
            call: bli_kvm_agent_protocol::Call::Ping,
            timeout_ms: 3_000,
            lease: None,
        }]
    );
}

#[tokio::test]
async fn a_refused_switch_is_reported_and_no_owner_means_unavailable() {
    use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};
    let root = tempdir().unwrap();
    let control = root.path().join("control.sock");
    super::quic_transport::owner(&control, |_| ControlResponse::Rejected {
        code: ControlRejectionCode::BackendRejected,
    });
    let app = app_with_options(ServerOptions {
        state_dir: root.path().join("account"),
        control_socket: Some(control),
        restricted_controls_enabled: true,
        ..ServerOptions::default()
    });
    let (cookie, csrf) = setup_and_login(&app).await;
    let refused = call(
        &app,
        &cookie,
        Some(&csrf),
        Method::POST,
        serde_json::json!({"enabled": true}),
    )
    .await;
    assert_ne!(refused["code"], 0);
    let state = call(&app, &cookie, None, Method::GET, Value::Null).await;
    assert_eq!(state["data"]["available"], false);

    let bare = tempdir().unwrap();
    let app = app_with_options(ServerOptions {
        state_dir: bare.path().join("account"),
        ..ServerOptions::default()
    });
    let (cookie, _) = setup_and_login(&app).await;
    let state = call(&app, &cookie, None, Method::GET, Value::Null).await;
    assert_eq!(state["data"]["available"], false);
    assert_eq!(state["data"]["enabled"], false);
}

#[tokio::test]
async fn an_agents_refusal_is_said_with_its_reason() {
    use bli_kvm_control_protocol::ControlResponse;
    let root = tempdir().unwrap();
    let control = root.path().join("control.sock");
    super::quic_transport::owner(&control, |_| ControlResponse::TargetAgentRefused {
        reason: "Windows refused the monitors' configuration (87)".into(),
    });
    let app = app_with_options(ServerOptions {
        state_dir: root.path().join("account"),
        control_socket: Some(control),
        restricted_controls_enabled: true,
        ..ServerOptions::default()
    });
    let (cookie, csrf) = setup_and_login(&app).await;
    let mut request = json_request(
        Method::POST,
        "/api/target-agent/call",
        serde_json::json!({"call": "ping"}),
    );
    request
        .headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    request
        .headers_mut()
        .insert("x-csrf-token", csrf.parse().unwrap());
    let refused = json_body(app.clone().oneshot(request).await.unwrap()).await;
    assert_ne!(refused["code"], 0);
    assert_eq!(
        refused["msg"], "the agent did not do it: Windows refused the monitors' configuration (87)",
        "{refused}"
    );
}
