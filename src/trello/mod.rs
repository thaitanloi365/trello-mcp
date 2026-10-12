//! Trello REST client: credentials as query parameters, a conservative rate
//! limit, retries for idempotent calls, and errors that never carry the URL.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use reqwest::{Method, StatusCode, Url, header};
use serde_json::{Value, json};

use crate::{
    config::{self, Config, Manager},
    util, version,
};

const MAX_RESPONSE_BYTES: usize = 16 << 20;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] config::Error),
    #[error("Trello {method} {path} returned HTTP {status}: {message}")]
    Api {
        method: Method,
        path: String,
        status: u16,
        message: String,
    },
    #[error("call Trello {method} {path}: {cause}")]
    Transport {
        method: Method,
        path: String,
        cause: reqwest::Error,
    },
    #[error("{0}")]
    Message(String),
}

fn message(text: impl Into<String>) -> Error {
    Error::Message(text.into())
}

enum Payload<'a> {
    Empty,
    Json(Vec<u8>),
    File { name: &'a str, data: &'a [u8] },
}

pub struct Client {
    config: Arc<Manager>,
    http: reqwest::Client,
    next: Mutex<Instant>,
}

impl Client {
    pub fn new(config: Arc<Manager>) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
            next: Mutex::new(Instant::now()),
        }
    }

    pub fn manager(&self) -> &Manager {
        &self.config
    }

    /// Spaces requests 105 ms apart to stay below Trello's per-token limit.
    async fn wait(&self) {
        let ready = {
            let mut next = self
                .next
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let ready = (*next).max(Instant::now());
            *next = ready + Duration::from_millis(105);
            ready
        };
        tokio::time::sleep_until(ready.into()).await;
    }

    pub async fn call(
        &self,
        method: Method,
        path: &str,
        params: Value,
        body: Option<&Value>,
    ) -> Result<Value, Error> {
        let cfg = self.config.effective()?;
        cfg.validate(true)?;
        let url = build_url(&cfg, path, &params)?;
        let payload = match body {
            Some(body) => Payload::Json(
                serde_json::to_vec(body)
                    .map_err(|err| message(format!("encode Trello request body: {err}")))?,
            ),
            None => Payload::Empty,
        };
        self.execute(&cfg, method, path, url, payload).await
    }

    async fn execute(
        &self,
        cfg: &Config,
        method: Method,
        safe_path: &str,
        url: Url,
        payload: Payload<'_>,
    ) -> Result<Value, Error> {
        let timeout = Duration::from_secs(cfg.request_timeout_seconds.max(1) as u64);
        for attempt in 0..3 {
            self.wait().await;
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .timeout(timeout)
                .header(header::ACCEPT, "application/json")
                .header(header::USER_AGENT, version::USER_AGENT);
            request = match &payload {
                Payload::Empty => request,
                Payload::Json(bytes) => request
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(bytes.clone()),
                Payload::File { name, data } => {
                    let part = reqwest::multipart::Part::bytes(data.to_vec())
                        .file_name(name.to_string())
                        .mime_str("application/octet-stream")
                        .map_err(|err| {
                            message(format!("create attachment form: {}", err.without_url()))
                        })?;
                    request.multipart(reqwest::multipart::Form::new().part("file", part))
                }
            };
            let response = match request.send().await {
                Ok(response) => response,
                Err(err) => {
                    if attempt < 2 && idempotent(&method) {
                        continue;
                    }
                    return Err(Error::Transport {
                        method,
                        path: safe_path.into(),
                        cause: err.without_url(),
                    });
                }
            };
            let status = response.status();
            let retry_after = response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            let data = read_limited(response).await?;
            let retryable = status == StatusCode::TOO_MANY_REQUESTS
                || (status.is_server_error() && idempotent(&method));
            if retryable && attempt < 2 {
                tokio::time::sleep(retry_delay(&retry_after, attempt)).await;
                continue;
            }
            if !status.is_success() {
                return Err(Error::Api {
                    method,
                    path: safe_path.into(),
                    status: status.as_u16(),
                    message: response_message(&data, status),
                });
            }
            if data.trim_ascii().is_empty() {
                return Ok(json!({"ok": true}));
            }
            return Ok(serde_json::from_slice(&data).unwrap_or_else(
                |_| json!({"ok": true, "response": String::from_utf8_lossy(&data)}),
            ));
        }
        unreachable!("the third attempt always returns")
    }

    pub async fn upload_file(&self, card_id: &str, path: &str, name: &str) -> Result<Value, Error> {
        let cfg = self.config.effective()?;
        let info =
            std::fs::metadata(path).map_err(|err| message(format!("open attachment: {err}")))?;
        if !info.is_file() {
            return Err(message("attachment path must be a regular file"));
        }
        let size = info.len() as i64;
        if size > cfg.max_upload_bytes {
            return Err(message(format!(
                "attachment is {size} bytes; configured limit is {}",
                cfg.max_upload_bytes
            )));
        }
        let data = std::fs::read(path).map_err(|err| message(format!("open attachment: {err}")))?;
        // The file can grow between the stat and the read.
        if data.len() as i64 > cfg.max_upload_bytes {
            return Err(message(format!(
                "attachment is {} bytes; configured limit is {}",
                data.len(),
                cfg.max_upload_bytes
            )));
        }
        let name = if name.is_empty() {
            Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            name.to_string()
        };
        self.upload(&cfg, card_id, &name, &data).await
    }

    pub async fn upload_data(
        &self,
        card_id: &str,
        name: &str,
        data: &[u8],
    ) -> Result<Value, Error> {
        let cfg = self.config.effective()?;
        if data.len() as i64 > cfg.max_upload_bytes {
            return Err(message(format!(
                "attachment is {} bytes; configured limit is {}",
                data.len(),
                cfg.max_upload_bytes
            )));
        }
        self.upload(&cfg, card_id, name, data).await
    }

    async fn upload(
        &self,
        cfg: &Config,
        card_id: &str,
        name: &str,
        data: &[u8],
    ) -> Result<Value, Error> {
        cfg.validate(true)?;
        let path = format!("/cards/{}/attachments", util::path_escape(card_id));
        let url = build_url(cfg, &path, &Value::Null)?;
        self.execute(cfg, Method::POST, &path, url, Payload::File { name, data })
            .await
    }

    pub async fn download_attachment(
        &self,
        card_id: &str,
        attachment_id: &str,
        destination: &str,
    ) -> Result<Value, Error> {
        let meta_path = format!(
            "/cards/{}/attachments/{}",
            util::path_escape(card_id),
            util::path_escape(attachment_id)
        );
        let metadata = self
            .call(Method::GET, &meta_path, Value::Null, None)
            .await?;
        let Value::Object(object) = metadata else {
            return Err(message("Trello returned invalid attachment metadata"));
        };
        let file_name = match object.get("name") {
            Some(Value::String(name)) if !name.trim().is_empty() => name.clone(),
            _ => attachment_id.to_string(),
        };
        let cfg = self.config.effective()?;
        let download_path = format!("{meta_path}/download/{}", util::path_escape(&file_name));
        let url = build_url(&cfg, &download_path, &Value::Null)?;
        let timeout = Duration::from_secs(cfg.request_timeout_seconds.max(1) as u64);
        let mut response = self
            .http
            .get(url)
            .timeout(timeout)
            .header(
                header::AUTHORIZATION,
                format!(
                    "OAuth oauth_consumer_key=\"{}\", oauth_token=\"{}\"",
                    cfg.api_key, cfg.token
                ),
            )
            .header(header::USER_AGENT, version::USER_AGENT)
            .send()
            .await
            .map_err(|err| message(format!("download Trello attachment: {}", err.without_url())))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Api {
                method: Method::GET,
                path: "/attachments/download".into(),
                status: status.as_u16(),
                message: status_text(status),
            });
        }
        let abs = util::absolute(Path::new(destination))
            .map_err(|err| message(format!("resolve attachment destination: {err}")))?;
        let parent = abs.parent().unwrap_or(Path::new("/")).to_path_buf();
        if !parent.is_dir() {
            return Err(message(format!(
                "attachment destination directory does not exist: {}",
                parent.display()
            )));
        }
        let mut tmp = tempfile::Builder::new()
            .prefix(".trello-attachment-")
            .suffix(".tmp")
            .tempfile_in(&parent)
            .map_err(|err| message(format!("create temporary attachment: {err}")))?;
        let limit = cfg.max_upload_bytes;
        let mut written: i64 = 0;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| message(format!("write attachment: {}", err.without_url())))?
        {
            written += chunk.len() as i64;
            if written > limit {
                return Err(message(format!(
                    "attachment exceeds configured limit of {limit} bytes"
                )));
            }
            std::io::Write::write_all(&mut tmp, &chunk)
                .map_err(|err| message(format!("write attachment: {err}")))?;
        }
        tmp.as_file()
            .sync_all()
            .map_err(|err| message(format!("sync attachment: {err}")))?;
        tmp.persist(&abs)
            .map_err(|err| message(format!("save attachment: {}", err.error)))?;
        util::set_mode(&abs, 0o600).map_err(|err| message(format!("secure attachment: {err}")))?;
        Ok(json!({"bytes": written, "name": file_name, "path": abs.display().to_string()}))
    }

    pub fn board_id(&self, explicit: &str) -> Result<String, Error> {
        let id = explicit.trim();
        if !id.is_empty() {
            return Ok(id.to_string());
        }
        let cfg = self.config.effective()?;
        if cfg.default_board_id.is_empty() {
            return Err(message(
                "boardId is required because no active board is configured",
            ));
        }
        Ok(cfg.default_board_id)
    }

    pub fn workspace_allowed(&self, id: &str) -> bool {
        match self.config.effective() {
            Err(_) => false,
            Ok(cfg) => {
                cfg.allowed_workspace_ids.is_empty()
                    || cfg
                        .allowed_workspace_ids
                        .iter()
                        .any(|allowed| allowed == id)
            }
        }
    }

    pub fn require_workspace_allowed(&self, id: &str) -> Result<(), Error> {
        if id.is_empty() {
            return Err(message("workspaceId is required"));
        }
        if !self.workspace_allowed(id) {
            return Err(message(format!(
                "workspace {id:?} is not allowed by configuration"
            )));
        }
        Ok(())
    }

    pub async fn ensure_board_allowed(&self, board_id: &str) -> Result<(), Error> {
        let cfg = self.config.effective()?;
        if cfg.allowed_workspace_ids.is_empty() {
            return Ok(());
        }
        let path = format!("/boards/{}", util::path_escape(board_id));
        let board = self
            .call(
                Method::GET,
                &path,
                json!({"fields": "id,idOrganization,name"}),
                None,
            )
            .await?;
        let Value::Object(object) = board else {
            return Err(message("Trello returned an invalid board response"));
        };
        let workspace = object
            .get("idOrganization")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !self.workspace_allowed(workspace) {
            return Err(message(format!(
                "board {board_id:?} belongs to workspace {workspace:?}, which is not allowed"
            )));
        }
        Ok(())
    }
}

fn build_url(cfg: &Config, path: &str, params: &Value) -> Result<Url, Error> {
    let raw = format!(
        "{}/{}",
        cfg.api_base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let mut url = Url::parse(&raw).map_err(|err| message(format!("build Trello URL: {err}")))?;
    let mut query = BTreeMap::new();
    query.insert("key".to_string(), cfg.api_key.clone());
    query.insert("token".to_string(), cfg.token.clone());
    if let Value::Object(params) = params {
        for (name, value) in params {
            if let Some(text) = query_value(value) {
                query.insert(name.clone(), text);
            }
        }
    }
    {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in &query {
            pairs.append_pair(name, value);
        }
    }
    Ok(url)
}

/// Go appendQueryValue: integral numbers print without a decimal point.
fn query_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(match (number.as_i64(), number.as_f64()) {
            (Some(int), _) => int.to_string(),
            (None, Some(float)) => float.to_string(),
            _ => number.to_string(),
        }),
        Value::Array(items) => Some(
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| item.to_string())
                })
                .collect::<Vec<_>>()
                .join(","),
        ),
        Value::Object(_) => Some(value.to_string()),
    }
}

async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>, Error> {
    let mut data = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| message(format!("read Trello response: {}", err.without_url())))?
    {
        data.extend_from_slice(&chunk);
        if data.len() > MAX_RESPONSE_BYTES {
            return Err(message("Trello response exceeded 16 MiB limit"));
        }
    }
    Ok(data)
}

fn idempotent(method: &Method) -> bool {
    [
        Method::GET,
        Method::HEAD,
        Method::OPTIONS,
        Method::PUT,
        Method::DELETE,
    ]
    .contains(method)
}

fn retry_delay(value: &str, attempt: u32) -> Duration {
    match value.trim().parse::<u64>() {
        Ok(seconds) if seconds > 0 => Duration::from_secs(seconds),
        _ => Duration::from_millis(250 << attempt),
    }
}

/// Go http.StatusText wording where it differs from the http crate.
fn status_text(status: StatusCode) -> String {
    match status.as_u16() {
        413 => "Request Entity Too Large",
        414 => "Request URI Too Long",
        416 => "Requested Range Not Satisfiable",
        _ => status.canonical_reason().unwrap_or(""),
    }
    .to_string()
}

fn response_message(data: &[u8], status: StatusCode) -> String {
    let text = String::from_utf8_lossy(data);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return status_text(status);
    }
    if let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(data) {
        for key in ["message", "error"] {
            if let Some(Value::String(value)) = object.get(key)
                && !value.is_empty()
            {
                return value.clone();
            }
        }
    }
    if trimmed.len() > 500 {
        return format!(
            "{}...",
            &trimmed[..(0..=500)
                .rev()
                .find(|i| trimmed.is_char_boundary(*i))
                .unwrap_or(0)]
        );
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, Config};
    use std::collections::HashMap;
    use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

    struct Harness {
        client: Client,
        api: MockServer,
        _dir: tempfile::TempDir,
    }

    async fn test_client(
        handler: impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static,
    ) -> Harness {
        let api = MockServer::start().await;
        Mock::given(any()).respond_with(handler).mount(&api).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = Config {
            api_key: "test-key".into(),
            token: "test-token".into(),
            api_base_url: api.uri(),
            ..Config::default()
        };
        config::save(&path, &cfg).unwrap();
        let manager =
            Arc::new(Manager::with_env(path.to_str().unwrap(), config::no_env()).unwrap());
        Harness {
            client: Client::new(manager),
            api,
            _dir: dir,
        }
    }

    fn query(request: &Request) -> HashMap<String, String> {
        request.url.query_pairs().into_owned().collect()
    }

    #[tokio::test]
    async fn call_adds_credentials_and_decodes_response() {
        let h = test_client(|_| ResponseTemplate::new(200).set_body_json(json!({"id": "card-1"})))
            .await;
        let result = h
            .client
            .call(Method::GET, "/cards/card-1", json!({"limit": 5}), None)
            .await
            .unwrap();
        assert_eq!(result["id"], "card-1");
        let requests = h.api.received_requests().await.unwrap();
        let q = query(&requests[0]);
        assert_eq!(q["key"], "test-key");
        assert_eq!(q["token"], "test-token");
        assert_eq!(q["limit"], "5");
        assert_eq!(requests[0].headers["user-agent"], version::USER_AGENT);
    }

    #[tokio::test]
    async fn integral_float_params_have_no_decimal_point() {
        let h = test_client(|_| ResponseTemplate::new(200).set_body_string("")).await;
        let result = h
            .client
            .call(
                Method::PUT,
                "/cards/c",
                json!({"dueReminder": 1440.0, "closed": true}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(result, json!({"ok": true}));
        let q = query(&h.api.received_requests().await.unwrap()[0]);
        assert_eq!(q["dueReminder"], "1440");
        assert_eq!(q["closed"], "true");
    }

    #[tokio::test]
    async fn api_error_does_not_leak_credentials() {
        let h = test_client(|_| ResponseTemplate::new(401).set_body_string("invalid key test-key"))
            .await;
        let err = h
            .client
            .call(Method::GET, "/members/me", Value::Null, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with("Trello GET /members/me returned HTTP 401: "),
            "{err}"
        );
        assert!(!err.contains("test-token"), "{err}");
    }

    #[tokio::test]
    async fn transport_error_does_not_leak_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = Config {
            api_key: "test-key".into(),
            token: "test-token".into(),
            api_base_url: "http://127.0.0.1:9".into(), // discard port: connection refused
            ..Config::default()
        };
        config::save(&path, &cfg).unwrap();
        let client = Client::new(Arc::new(
            Manager::with_env(path.to_str().unwrap(), config::no_env()).unwrap(),
        ));
        let err = client
            .call(Method::POST, "/cards", Value::Null, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("call Trello POST /cards: "), "{err}");
        assert!(
            !err.contains("test-key") && !err.contains("test-token"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn non_idempotent_request_is_not_retried_after_server_error() {
        let h =
            test_client(|_| ResponseTemplate::new(500).set_body_json(json!({"message": "boom"})))
                .await;
        let err = h
            .client
            .call(Method::POST, "/cards", Value::Null, None)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(err, "Trello POST /cards returned HTTP 500: boom");
        assert_eq!(h.api.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn download_attachment_uses_authenticated_download_endpoint() {
        let h = test_client(|request: &Request| match request.url.path() {
            "/cards/card-1/attachments/att-1" => ResponseTemplate::new(200)
                .set_body_json(json!({"id": "att-1", "name": "spec v2.pdf"})),
            "/cards/card-1/attachments/att-1/download/spec%20v2.pdf" => {
                ResponseTemplate::new(200).set_body_bytes(b"PDF".to_vec())
            }
            _ => ResponseTemplate::new(404),
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("out.pdf");
        let result = h
            .client
            .download_attachment("card-1", "att-1", destination.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["bytes"], 3);
        assert_eq!(result["name"], "spec v2.pdf");
        assert_eq!(std::fs::read(&destination).unwrap(), b"PDF");
        let requests = h.api.received_requests().await.unwrap();
        let download = &requests[1];
        assert_eq!(
            download.headers["authorization"],
            "OAuth oauth_consumer_key=\"test-key\", oauth_token=\"test-token\""
        );
    }

    #[test]
    fn response_message_truncates_on_char_boundary() {
        let body = "é".repeat(600);
        let message = response_message(body.as_bytes(), StatusCode::BAD_REQUEST);
        assert!(message.ends_with("..."));
        assert!(message.len() <= 503);
        assert_eq!(response_message(b"", StatusCode::NOT_FOUND), "Not Found");
        assert_eq!(
            response_message(b"", StatusCode::PAYLOAD_TOO_LARGE),
            "Request Entity Too Large"
        );
        assert_eq!(
            response_message(b"", StatusCode::URI_TOO_LONG),
            "Request URI Too Long"
        );
        assert_eq!(
            response_message(b"", StatusCode::RANGE_NOT_SATISFIABLE),
            "Requested Range Not Satisfiable"
        );
        assert_eq!(
            response_message(br#"{"error":"nope"}"#, StatusCode::BAD_REQUEST),
            "nope"
        );
    }
}
