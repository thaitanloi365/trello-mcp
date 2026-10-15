//! `trello-mcp dashboard`: the tray's server table plus Trello search, the @mentions waiting on
//! you, and the "Ready for dev" and "In progress" lists, as a local web page. Two loopback
//! listeners, as in gws-mcp: 127.0.0.1:8790 for this Mac, and with --tailscale a second one that
//! `tailscale serve` points at, open only to the owner's Tailscale login.

#[cfg(target_os = "macos")]
mod procs;
mod servers;

use std::{
    collections::{HashMap, HashSet},
    future::IntoFuture,
    net::Ipv4Addr,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use axum::{
    Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};

use crate::{
    config,
    mcp::{
        self,
        feeds::{self, ListKind},
    },
    trello,
};

pub(crate) const ADDR: &str = "127.0.0.1:8790";
pub(crate) const TAILNET_ADDR: &str = "127.0.0.1:8791";
pub(crate) const PAGE: &[u8] = include_bytes!("../../web/index.html");
const TRELLO_TTL: Duration = Duration::from_secs(60);

/// What the scans know; one scan at a time, under the lock.
#[derive(Default)]
struct Servers {
    rows: Vec<servers::Row>,
    scanned_at: Option<f64>,
    stopping: HashSet<servers::ServerId>,
    #[cfg(target_os = "macos")]
    scanner: procs::Scanner,
}

struct TrelloView {
    at: Instant,
    boards: Arc<Vec<Value>>,
    body: Value,
}

pub(crate) struct Dash {
    hosts: HashSet<String>,
    origins: HashSet<String>,
    ts_host: String,
    ts_login: String,
    trello: Result<mcp::Server, String>,
    cli: String,
    home: String,
    servers: Mutex<Servers>,
    trello_view: tokio::sync::Mutex<Option<TrelloView>>,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

impl Dash {
    /// `cli` is this binary: its servers show as "installed". `tailscale` is "" or host:port=login.
    pub(crate) fn new(
        trello: Result<mcp::Server, String>,
        cli: String,
        port: u16,
        tailscale: &str,
    ) -> Arc<Self> {
        let hosts: HashSet<String> =
            [format!("127.0.0.1:{port}"), format!("localhost:{port}")].into();
        let mut origins: HashSet<String> = hosts.iter().map(|h| format!("http://{h}")).collect();
        let (ts_host, ts_login) = tailscale.split_once('=').unwrap_or((tailscale, ""));
        if !ts_host.is_empty() && !ts_login.is_empty() {
            origins.insert(format!("https://{ts_host}"));
        }
        Arc::new(Self {
            hosts,
            origins,
            ts_host: ts_host.into(),
            ts_login: ts_login.into(),
            trello,
            cli,
            home: dirs::home_dir()
                .map(|h| h.to_string_lossy().into_owned())
                .unwrap_or_default(),
            servers: Mutex::new(Servers::default()),
            trello_view: tokio::sync::Mutex::new(None),
        })
    }

    /// Scans unless the last scan is under `min_gap` s old. Blocking: call it on the blocking pool.
    fn scan(&self, min_gap: f64) {
        let mut s = self.servers.lock().unwrap();
        let at = now();
        if s.scanned_at.is_some_and(|t| at - t < min_gap) {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let found = s.scanner.scan();
            let live: HashSet<servers::ServerId> = found.iter().map(|f| f.id).collect();
            s.stopping.retain(|id| live.contains(id)); // a stopped server that is gone is done stopping
            let prev = servers::baseline(s.scanned_at, at);
            s.rows = servers::advance(&s.rows, found, prev, at);
        }
        s.scanned_at = Some(at);
    }

    /// Lets only our page act: another site cannot send the custom header without a CORS
    /// preflight we never answer, and a present Origin must be ours.
    fn is_ours(&self, h: &HeaderMap) -> bool {
        let origin = hdr(h, header::ORIGIN);
        hdr(h, "x-trello-mcp") == "1" && (origin.is_empty() || self.origins.contains(&origin))
    }

    /// The boards in scope (snapshots) and the page's Trello sections, from a 60 s cache.
    async fn trello_view(&self, fresh: bool) -> (Arc<Vec<Value>>, Value) {
        let mut cache = self.trello_view.lock().await;
        if let Some(c) = cache
            .as_ref()
            .filter(|c| !fresh && c.at.elapsed() < TRELLO_TTL)
        {
            return (c.boards.clone(), c.body.clone());
        }
        let (boards, body) = match &self.trello {
            Ok(server) => load_trello(server).await,
            Err(e) => (Vec::new(), json!({"error": e})),
        };
        let boards = Arc::new(boards);
        // An error is not cached: the next load tries again.
        *cache = (body.get("error").is_none()).then(|| TrelloView {
            at: Instant::now(),
            boards: boards.clone(),
            body: body.clone(),
        });
        (boards, body)
    }
}

/// Every board's snapshot, then the sections; a board that fails is named, the rest still show.
async fn load_trello(server: &mcp::Server) -> (Vec<Value>, Value) {
    let now = now() as i64;
    let scope = match server.list_boards("").await {
        Ok(v) => mcp::object_slice(&v),
        Err(e) => return (Vec::new(), json!({"error": e.to_string()})),
    };
    let (mut boards, mut failed) = (Vec::new(), Vec::new());
    for b in &scope {
        let id = b.get("id").and_then(Value::as_str).unwrap_or_default();
        match server.board_snapshot(id).await {
            Ok(v) => boards.push(v),
            Err(e) => failed.push(format!(
                "{}: {e}",
                b.get("name").and_then(Value::as_str).unwrap_or(id)
            )),
        }
    }
    let lists_error = (!failed.is_empty()).then(|| {
        format!(
            "Could not read {} board(s): {}",
            failed.len(),
            failed.join("; ")
        )
    });
    let section = |kind| {
        let mut v = json!({"boards": feeds::named_lists(&boards, kind, now)});
        if let Some(e) = &lists_error {
            v["error"] = json!(e);
        }
        v
    };
    let (ready, progress) = (
        section(ListKind::ReadyForDev),
        section(ListKind::InProgress),
    );
    let waiting = match server.waiting(&boards, now).await {
        Ok(rows) => json!({"rows": rows}),
        Err(e) => json!({"rows": [], "error": e.to_string()}),
    };
    let body = json!({"boards": boards.len(), "updatedAt": now, "waiting": waiting, "ready": ready, "progress": progress});
    (boards, body)
}

/// Serves the dashboard on `addr` until SIGTERM or Ctrl-C. With `tailscale` set, a second loopback
/// listener on `tailnet_addr` serves the tailnet: `tailscale serve` points there.
pub(crate) async fn run(
    config_path: &str,
    addr: &str,
    tailnet_addr: &str,
    tailscale: &str,
) -> Result<()> {
    let port = check_addr(addr)?;
    check_tailscale(tailscale)?;
    // The bound port is the single-instance lock.
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .with_context(|| format!("listen {addr}: is the dashboard already running?"))?;
    let tailnet = if tailscale.is_empty() {
        None
    } else {
        let p = check_addr(tailnet_addr)?;
        Some(
            tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, p))
                .await
                .with_context(|| format!("listen {tailnet_addr} (--tailnet-addr)"))?,
        )
    };
    // A bad config still serves the server table; the Trello sections show why.
    let trello = config::Manager::new(config_path)
        .map(|m| mcp::Server::new(Arc::new(trello::Client::new(Arc::new(m)))))
        .map_err(|e| e.to_string());
    let cli = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dash = Dash::new(trello, cli, port, tailscale);
    // A scan a minute, so ACTIVE knows when each server last worked.
    let d = dash.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            let d = d.clone();
            let _ = tokio::task::spawn_blocking(move || d.scan(0.0)).await;
        }
    });
    eprintln!("trello-mcp dashboard: http://{addr}");
    let local = axum::serve(listener, router(dash.clone(), false))
        .with_graceful_shutdown(crate::shutdown_signal())
        .into_future();
    match tailnet {
        None => local.await?,
        Some(l) => {
            eprintln!("trello-mcp dashboard (tailnet): http://{tailnet_addr}");
            let ts = axum::serve(l, router(dash, true)).into_future();
            tokio::select! {
                r = local => r?,
                r = ts => r?,
            }
        }
    }
    Ok(())
}

/// The port of `127.0.0.1:<port>` or `localhost:<port>`: the dashboard answers on loopback only.
fn check_addr(addr: &str) -> Result<u16> {
    addr.rsplit_once(':')
        .filter(|(host, _)| *host == "127.0.0.1" || *host == "localhost")
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .filter(|p| *p > 0)
        .ok_or_else(|| anyhow!("--addr must be 127.0.0.1:<port> or localhost:<port>, got {addr:?}"))
}

/// "" (off) or host:port=login.
fn check_tailscale(ts: &str) -> Result<()> {
    match ts.split_once('=') {
        _ if ts.is_empty() => Ok(()),
        Some((host, login)) if !host.is_empty() && !login.is_empty() => Ok(()),
        _ => bail!("--tailscale must be host:port=login, got {ts:?}"),
    }
}

/// The local listener (`tailnet` false) or the one `tailscale serve` points at (true).
pub(crate) fn router(dash: Arc<Dash>, tailnet: bool) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], PAGE) }),
        )
        .route("/api/servers", get(servers_now))
        .route("/api/servers/{pid}/{start}/stop", post(stop_server))
        .route("/api/trello", get(trello_now))
        .route("/api/search", get(search))
        .fallback(|| async { http_error(StatusCode::NOT_FOUND, "404 page not found") })
        .layer(middleware::from_fn_with_state(
            (dash.clone(), tailnet),
            guard,
        ))
        .with_state(dash)
}

/// Every route, never framed or cached. Trust follows the listener, not the Host a client sends:
/// the local one takes loopback Hosts and no proxy; the tailnet one takes the Tailscale Host and
/// the owner's login (`tailscale serve` replaces any Tailscale-User-Login a client sends).
async fn guard(
    State((dash, tailnet)): State<(Arc<Dash>, bool)>,
    req: Request,
    next: Next,
) -> Response {
    let h = req.headers();
    let host = hdr(h, header::HOST);
    let allowed = if tailnet {
        !dash.ts_login.is_empty()
            && host == dash.ts_host
            && hdr(h, "tailscale-user-login") == dash.ts_login
    } else {
        dash.hosts.contains(&host) && !h.keys().any(|k| k.as_str().starts_with("x-forwarded-"))
    };
    if !allowed {
        return http_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

async fn servers_now(State(dash): State<Arc<Dash>>) -> Response {
    if !cfg!(target_os = "macos") {
        return json_ok(&json!({"unsupported": true}));
    }
    let d = dash.clone();
    let _ = tokio::task::spawn_blocking(move || d.scan(1.0)).await;
    let s = dash.servers.lock().unwrap();
    json_ok(&servers::view(
        &s.rows,
        &s.stopping,
        now(),
        &dash.cli,
        &dash.home,
    ))
}

/// Quit's Confirm: SIGTERM now, SIGKILL after 5 s; only for a server the last scan listed.
async fn stop_server(
    State(dash): State<Arc<Dash>>,
    h: HeaderMap,
    Path((pid, start)): Path<(i32, u64)>,
) -> Response {
    if !dash.is_ours(&h) {
        return http_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let id = servers::ServerId { pid, start };
    if !dash
        .servers
        .lock()
        .unwrap()
        .rows
        .iter()
        .any(|r| r.s.id == id)
    {
        return http_error(StatusCode::NOT_FOUND, "no such server");
    }
    #[cfg(target_os = "macos")]
    {
        match procs::terminate(id, procs::SERVER_NAME) {
            Err(e) => return http_error(StatusCode::FORBIDDEN, &e),
            Ok(false) => return http_error(StatusCode::NOT_FOUND, "no such server"),
            Ok(true) => {
                eprintln!("trello-mcp dashboard: stopping PID {pid}");
                dash.servers.lock().unwrap().stopping.insert(id);
                tokio::spawn(async move {
                    tokio::time::sleep(procs::STOP_GRACE).await;
                    procs::kill_if_alive(id, procs::SERVER_NAME);
                });
            }
        }
    }
    StatusCode::ACCEPTED.into_response()
}

async fn trello_now(
    State(dash): State<Arc<Dash>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    json_ok(&dash.trello_view(q.contains_key("fresh")).await.1)
}

async fn search(
    State(dash): State<Arc<Dash>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let q = q.get("q").map_or("", String::as_str);
    if let Err(e) = feeds::classify(q) {
        return http_error(StatusCode::BAD_REQUEST, &e.to_string());
    }
    let server = match &dash.trello {
        Ok(server) => server,
        Err(e) => return json_ok(&json!({"error": e})),
    };
    let (boards, body) = dash.trello_view(false).await;
    // A total failure must not read as "no matches".
    if let Some(e) = body.get("error") {
        return json_ok(&json!({"error": e}));
    }
    match server.search_cards(q, &boards, now() as i64).await {
        Ok(cards) => json_ok(&json!({"cards": cards})),
        Err(e) => json_ok(&json!({"error": e.to_string()})),
    }
}

fn json_ok(v: &Value) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], v.to_string()).into_response()
}

fn http_error(status: StatusCode, msg: &str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        format!("{msg}\n"),
    )
        .into_response()
}

fn hdr(h: &HeaderMap, name: impl header::AsHeaderName) -> String {
    h.get(name)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use serde_json::json;

    use super::*;
    use crate::mcp::test_support::{calls, json_response, not_found, test_server};

    /// A dashboard on 127.0.0.1:<random port>, and its tailnet listener on another.
    async fn start(trello: Result<mcp::Server, String>, tailscale: &str) -> (String, String) {
        let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = local.local_addr().unwrap().port();
        let dash = Dash::new(trello, "/x/trello-mcp".into(), port, tailscale);
        let ts = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ts_base = format!("127.0.0.1:{}", ts.local_addr().unwrap().port());
        let (a, b) = (router(dash.clone(), false), router(dash, true));
        tokio::spawn(async move { axum::serve(local, a).await.unwrap() });
        tokio::spawn(async move { axum::serve(ts, b).await.unwrap() });
        (format!("127.0.0.1:{port}"), ts_base)
    }

    /// (status, raw response) of one HTTP/1.1 request sent as written, so the test picks the Host.
    async fn call(to: &str, method: &str, path: &str, headers: &[(&str, &str)]) -> (u16, String) {
        let mut req =
            format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: 0\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        let to = to.to_string();
        // A blocking socket on the blocking pool: the server runs on this test's runtime.
        tokio::task::spawn_blocking(move || {
            let mut s = std::net::TcpStream::connect(to).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            (out[9..12].parse().unwrap(), out)
        })
        .await
        .unwrap()
    }

    fn body(raw: &str) -> &str {
        raw.split_once("\r\n\r\n").map_or("", |(_, b)| b)
    }

    async fn get_json(to: &str, path: &str) -> Value {
        let (_, raw) = call(to, "GET", path, &[("Host", to)]).await;
        serde_json::from_str(body(&raw)).unwrap_or_else(|e| panic!("{e}: {raw}"))
    }

    #[test]
    fn flags_are_checked() {
        assert_eq!(check_addr("127.0.0.1:8790").unwrap(), 8790);
        assert_eq!(check_addr("localhost:1").unwrap(), 1);
        assert!(
            check_addr("0.0.0.0:8790").is_err()
                && check_addr("127.0.0.1:0").is_err()
                && check_addr("8790").is_err()
        );
        assert!(check_tailscale("").is_ok() && check_tailscale("h.ts.net:8448=me@x.io").is_ok());
        assert!(
            check_tailscale("h.ts.net:8448").is_err()
                && check_tailscale("=me").is_err()
                && check_tailscale("h:1=").is_err()
        );
    }

    #[tokio::test]
    async fn local_listener_takes_loopback_hosts_only() {
        let (us, _) = start(Err("no config".into()), "").await;
        let (status, page) = call(&us, "GET", "/", &[("Host", &us)]).await;
        let low = page.to_lowercase();
        assert_eq!(status, 200);
        assert!(
            low.contains("x-frame-options: deny")
                && low.contains("frame-ancestors 'none'")
                && low.contains("cache-control: no-store"),
            "{page}"
        );
        assert!(page.contains("Trello MCP"));
        assert_eq!(
            call(
                &us,
                "GET",
                "/",
                &[("Host", &us.replace("127.0.0.1", "localhost"))]
            )
            .await
            .0,
            200
        );
        assert_eq!(
            call(&us, "GET", "/", &[("Host", "evil.example:80")])
                .await
                .0,
            403
        );
        assert_eq!(
            call(
                &us,
                "GET",
                "/",
                &[("Host", &us), ("X-Forwarded-For", "100.64.0.1")]
            )
            .await
            .0,
            403,
            "never behind a proxy"
        );
        assert_eq!(call(&us, "GET", "/nope", &[("Host", &us)]).await.0, 404);
    }

    #[tokio::test]
    async fn tailnet_listener_takes_the_owner_only() {
        let (us, ts) = start(
            Err("no config".into()),
            "mac.example.ts.net:8448=me@example.com",
        )
        .await;
        let host = ("Host", "mac.example.ts.net:8448");
        let owner = ("Tailscale-User-Login", "me@example.com");
        assert_eq!(call(&ts, "GET", "/", &[host, owner]).await.0, 200);
        assert_eq!(
            call(
                &ts,
                "GET",
                "/",
                &[host, ("Tailscale-User-Login", "other@example.com")]
            )
            .await
            .0,
            403
        );
        assert_eq!(call(&ts, "GET", "/", &[host]).await.0, 403);
        assert_eq!(call(&ts, "GET", "/", &[("Host", &us), owner]).await.0, 403);
        assert_eq!(
            call(&us, "GET", "/", &[host, owner]).await.0,
            403,
            "the local listener takes no tailnet Host"
        );
    }

    #[tokio::test]
    async fn stop_needs_our_page_and_a_listed_server() {
        let (us, _) = start(Err("no config".into()), "").await;
        let origin = format!("http://{us}");
        let path = format!("/api/servers/{}/1/stop", std::process::id());
        let mine = [
            ("Host", us.as_str()),
            ("X-Trello-MCP", "1"),
            ("Origin", origin.as_str()),
        ];
        assert_eq!(
            call(&us, "POST", &path, &mine[..1]).await.0,
            403,
            "no X-Trello-MCP header"
        );
        assert_eq!(
            call(
                &us,
                "POST",
                &path,
                &[mine[0], mine[1], ("Origin", "https://evil.example")]
            )
            .await
            .0,
            403
        );
        assert_eq!(
            call(&us, "POST", &path, &mine).await.0,
            404,
            "not a listed server: nothing is signalled"
        );
        assert_eq!(
            call(&us, "POST", "/api/servers/1/1/stop", &mine).await.0,
            404
        );
    }

    #[tokio::test]
    async fn servers_answer_without_trello_config() {
        let (us, _) = start(Err("read config: api_key is required".into()), "").await;
        let v = get_json(&us, "/api/servers").await;
        if cfg!(target_os = "macos") {
            assert!(
                v["figures"]["servers"].is_string() && v["groups"].is_array(),
                "{v}"
            );
        } else {
            assert_eq!(v, json!({"unsupported": true}));
        }
        assert_eq!(
            get_json(&us, "/api/trello").await,
            json!({"error": "read config: api_key is required"})
        );
        let long = format!("/api/search?q={}", "a".repeat(201));
        assert_eq!(call(&us, "GET", &long, &[("Host", &us)]).await.0, 400);
        assert_eq!(
            get_json(&us, "/api/search?q=x").await,
            json!({"error": "read config: api_key is required"})
        );
    }

    #[tokio::test]
    async fn trello_sections_are_cached_for_a_minute() {
        let h = test_server(&[], |req| match req.url.path() {
            "/members/me/boards" => json_response(json!([
                {"id": "b1", "name": "Client Portal", "idOrganization": "w1"},
                {"id": "b2", "name": "Gone Board", "idOrganization": "w1"}
            ])),
            "/boards/b1" => json_response(feeds::tests::board()),
            "/members/me/notifications" | "/members/me/actions" => json_response(json!([])),
            _ => not_found(),
        })
        .await;
        let (us, _) = start(Ok(h.server.clone()), "").await;
        let v = get_json(&us, "/api/trello").await;
        assert_eq!(v["boards"], 1, "{v}");
        assert!(v["updatedAt"].as_i64().unwrap() > 1_700_000_000);
        assert_eq!(v["progress"]["boards"][0]["cards"][0]["number"], 348);
        assert_eq!(
            v["ready"]["boards"][0]["cards"].as_array().unwrap().len(),
            2
        );
        assert!(
            v["ready"]["error"].as_str().unwrap().contains("Gone Board"),
            "a failed board is named: {v}"
        );
        assert_eq!(v["waiting"], json!({"rows": []}));
        let n = calls(&h.api).await.len();
        get_json(&us, "/api/trello").await;
        assert_eq!(
            calls(&h.api).await.len(),
            n,
            "a second load within a minute comes from the cache"
        );
        get_json(&us, "/api/trello?fresh=1").await;
        assert!(calls(&h.api).await.len() > n, "Refresh refetches");
        let found = get_json(&us, "/api/search?q=%23348").await;
        assert_eq!(found["cards"][0]["name"], "Block physical address");
    }

    #[tokio::test]
    async fn search_reports_a_trello_failure() {
        let h = test_server(&[], |_| {
            wiremock::ResponseTemplate::new(401).set_body_string("invalid token")
        })
        .await;
        let (us, _) = start(Ok(h.server.clone()), "").await;
        let v = get_json(&us, "/api/search?q=%23348").await;
        assert!(v["error"].is_string() && v.get("cards").is_none(), "{v}");
    }

    #[tokio::test]
    async fn a_second_dashboard_on_the_same_port_fails() {
        let held = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = held.local_addr().unwrap().to_string();
        let err = run("", &addr, TAILNET_ADDR, "").await.unwrap_err();
        assert!(format!("{err:#}").contains("already running"), "{err:#}");
    }
}
