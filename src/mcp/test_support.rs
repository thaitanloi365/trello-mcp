//! Test harness: a wiremock stand-in for Trello and a Server pointed at it.

use std::{collections::HashMap, sync::Arc};

use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

use super::Server;
use crate::{
    config::{self, Config, Manager},
    trello::Client,
};

pub(crate) struct Harness {
    pub server: Server,
    pub manager: Arc<Manager>,
    pub api: MockServer,
    pub dir: tempfile::TempDir,
}

/// Assert on recorded requests after the call, not inside the responder: a
/// panic there only surfaces as a connection error.
pub(crate) async fn test_server(
    allowed: &[&str],
    handler: impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static,
) -> Harness {
    let api = MockServer::start().await;
    Mock::given(any()).respond_with(handler).mount(&api).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.json");
    let mut cfg = Config::default();
    cfg.api_key = "key".into();
    cfg.token = "token".into();
    cfg.api_base_url = api.uri();
    cfg.allowed_workspace_ids = allowed.iter().map(|id| id.to_string()).collect();
    config::save(&path, &cfg).expect("save config");
    let manager = Arc::new(
        Manager::with_env(path.to_str().expect("utf-8 path"), config::no_env()).expect("manager"),
    );
    Harness {
        server: Server::new(Arc::new(Client::new(manager.clone()))),
        manager,
        api,
        dir,
    }
}

pub(crate) fn json_response(value: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(value)
}

pub(crate) fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_string("404 page not found\n")
}

pub(crate) fn query(request: &Request) -> HashMap<String, String> {
    request.url.query_pairs().into_owned().collect()
}

pub(crate) async fn calls(api: &MockServer) -> Vec<String> {
    api.received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect()
}

pub(crate) fn board_fixture() -> Value {
    json!({
        "id": "board-1", "name": "Board",
        "lists": [
            {"id": "list-todo", "name": "To Do"},
            {"id": "list-done", "name": "Done"},
            {"id": "list-old", "name": "Done", "closed": true}
        ],
        "labels": [
            {"id": "label-bug", "name": "Bug", "color": "red"},
            {"id": "label-blue", "name": "", "color": "blue"}
        ],
        "members": [{"id": "member-joe", "fullName": "Joe Thai", "username": "joe"}]
    })
}
