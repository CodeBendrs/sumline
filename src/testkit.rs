//! Shared helpers for router-level tests: an app on a temporary store, a capture mailer, and a
//! minimal cookie/CSRF-aware client that talks to the real router.

use crate::{App, Config, outbound::Mailer, outbound::Sent, router, storage::Store};
use axum::{body::Body, http::Request, http::StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tower::ServiceExt;

pub const ORIGIN: &str = "http://localhost:3030";
pub const PASSWORD: &str = "a long enough password";

pub struct Fixture {
    pub app: App,
    pub outbox: Arc<Mutex<Vec<Sent>>>,
    pub _dir: tempfile::TempDir,
}
pub fn fixture(mail: bool) -> Fixture {
    fixture_with(mail, |_| {})
}
/// A fixture whose configuration the test may adjust (origin, signup code ...).
pub fn fixture_with(mail: bool, tweak: impl FnOnce(&mut Config)) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let outbox = Arc::new(Mutex::new(Vec::new()));
    let mut config = Config {
        origin: ORIGIN.into(),
        demo: false,
        samples: None,
        trusted_proxies: Vec::new(),
        signup_code: None,
        operator: crate::legal::Operator::default(),
        gmail_api: crate::mail::GMAIL_API.into(),
        graph_api: crate::mail::GRAPH_API.into(),
    };
    tweak(&mut config);
    let app = App {
        store: Store::open(dir.path()).unwrap(),
        config: Arc::new(config),
        http: reqwest::Client::new(),
        queue: Arc::new(tokio::sync::Notify::new()),
        sync_locks: Arc::new(Mutex::new(HashSet::new())),
        auth_attempts: Arc::new(Mutex::new(HashMap::new())),
        mailer: if mail {
            Mailer::Capture(outbox.clone())
        } else {
            Mailer::Disabled
        },
    };
    Fixture {
        app,
        outbox,
        _dir: dir,
    }
}

#[derive(Default, Clone)]
pub struct Session {
    pub cookie: String,
    pub csrf: String,
}
/// Sends a request through the real router and returns the untouched response.
pub async fn raw(
    app: &App,
    method: &str,
    path: &str,
    body: Option<Value>,
    session: Option<&Session>,
) -> axum::response::Response {
    let host = app
        .config
        .origin
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_string();
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("origin", app.config.origin.as_str())
        .header("content-type", "application/json");
    if let Some(session) = session {
        request = request
            .header("cookie", &session.cookie)
            .header("x-csrf-token", &session.csrf);
    }
    let body = body
        .map(|v| Body::from(v.to_string()))
        .unwrap_or_else(Body::empty);
    router(app.clone())
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}
pub async fn call(
    app: &App,
    method: &str,
    path: &str,
    body: Option<Value>,
    session: Option<&Session>,
) -> (StatusCode, Value, Option<String>) {
    let response = raw(app, method, path, body, session).await;
    let status = response.status();
    let cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::to_string);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        cookie,
    )
}
pub async fn register(app: &App, email: &str) -> Session {
    let (status, body, cookie) = call(
        app,
        "POST",
        "/api/auth/register",
        Some(json!({"email":email,"password":PASSWORD,"store_name":"Test","consent":true,"policy_version":crate::legal::POLICY_VERSION})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    Session {
        cookie: cookie.unwrap(),
        csrf: body["csrf"].as_str().unwrap().to_string(),
    }
}
pub async fn login(app: &App, email: &str, password: &str) -> (StatusCode, Option<Session>) {
    let (status, body, cookie) = call(
        app,
        "POST",
        "/api/auth/login",
        Some(json!({"email":email,"password":password})),
        None,
    )
    .await;
    let session = cookie.map(|cookie| Session {
        cookie,
        csrf: body["csrf"].as_str().unwrap_or("").to_string(),
    });
    (status, session)
}
/// Registration sends mail from a spawned task; wait for it.
pub async fn mail_count(outbox: &Arc<Mutex<Vec<Sent>>>, expected: usize) -> Vec<Sent> {
    for _ in 0..100 {
        if outbox.lock().unwrap().len() >= expected {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    outbox.lock().unwrap().clone()
}
pub fn token_in(message: &Sent, key: &str) -> String {
    let marker = format!("/?{key}=");
    let start = message.body.find(&marker).expect("link in message") + marker.len();
    message.body[start..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect()
}
pub async fn me(app: &App, session: &Session) -> Value {
    let (status, body, _) = call(app, "GET", "/api/me", None, Some(session)).await;
    assert_eq!(status, StatusCode::OK);
    body
}
