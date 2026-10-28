//! `trello-mcp dashboard`: the tray's server table plus Trello search, the @mentions waiting on
//! you, and one project's "Ready for dev" and "In progress" lists with Send to Orca, as a local
//! web page. Two loopback
//! listeners, as in gws-mcp: 127.0.0.1:8790 for this Mac, and with --tailscale a second one that
//! `tailscale serve` points at, open only to the owner's Tailscale login.

mod orca;
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
// Search, a project's lists and Send to Orca need only a load's board list: one this old will do.
const SEARCH_TTL: Duration = Duration::from_secs(600);

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
    /// The boards in scope: id, name, workspace.
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
    orca: String,
    /// One Send to Orca at a time: a double click must not make two worktrees.
    sending: tokio::sync::Mutex<()>,
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
    /// `cli` is this binary: its servers show as "installed"; `orca` is the Orca CLI.
    /// `tailscale` is "" or host:port=login.
    pub(crate) fn new(
        trello: Result<mcp::Server, String>,
        cli: String,
        orca: String,
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
            orca,
            sending: tokio::sync::Mutex::new(()),
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

    /// The boards in scope and the page's Trello view; a cached load no older than `max_age` is
    /// reused (`Duration::ZERO` always reloads).
    async fn trello_view(&self, max_age: Duration) -> (Arc<Vec<Value>>, Value) {
        let mut cache = self.trello_view.lock().await;
        if let Some(c) = cache.as_ref().filter(|c| c.at.elapsed() < max_age) {
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

/// The board ids of a load's board list.
fn ids(boards: &[Value]) -> Vec<String> {
    boards
        .iter()
        .map(|b| mcp::nested_string(b, &["id"]))
        .collect()
}

/// The boards in scope and the @mentions waiting on you; no board's cards: the page loads one
/// project at a time.
async fn load_trello(server: &mcp::Server) -> (Vec<Value>, Value) {
    let now = now() as i64;
    let boards = match server.list_boards("").await {
        Ok(v) => mcp::object_slice(&v)
            .into_iter()
            .map(Value::Object)
            .collect::<Vec<_>>(),
        Err(e) => return (Vec::new(), json!({"error": e.to_string()})),
    };
    let waiting = match server.waiting(&ids(&boards), now).await {
        Ok(rows) => json!({"rows": rows}),
        Err(e) => json!({"rows": [], "error": e.to_string()}),
    };
    let body = json!({"boards": boards, "updatedAt": now, "waiting": waiting});
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
    let dash = Dash::new(trello, cli, orca::cli(), port, tailscale);
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
        .route("/api/board", get(board_now))
        .route("/api/search", get(search))
        .route("/api/orca/repos", get(orca_repos))
        .route("/api/orca/sessions", get(orca_sessions))
        .route("/api/orca/send", post(orca_send))
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
    h: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    // Each load is several Trello calls on a token shared with the MCP servers: only our page may ask.
    if !dash.is_ours(&h) {
        return http_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let max_age = if q.contains_key("fresh") {
        Duration::ZERO
    } else {
        TRELLO_TTL
    };
    json_ok(&dash.trello_view(max_age).await.1)
}

/// The board list of a load, or the load's error.
async fn scope(dash: &Dash) -> Result<Arc<Vec<Value>>, Value> {
    let (boards, body) = dash.trello_view(SEARCH_TTL).await;
    match body.get("error") {
        Some(e) => Err(json!({"error": e})),
        None => Ok(boards),
    }
}

/// One project's "Ready for dev" and "In progress" cards.
async fn board_now(
    State(dash): State<Arc<Dash>>,
    h: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !dash.is_ours(&h) {
        return http_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let server = match &dash.trello {
        Ok(server) => server,
        Err(e) => return json_ok(&json!({"error": e})),
    };
    let boards = match scope(&dash).await {
        Ok(b) => b,
        Err(e) => return json_ok(&e),
    };
    let id = q.get("id").map_or("", String::as_str);
    if !ids(&boards).iter().any(|b| b == id) {
        return http_error(StatusCode::NOT_FOUND, "no such board");
    }
    let now = now() as i64;
    match server.board_snapshot(id).await {
        Ok(b) => {
            let b = [b];
            let section = |kind| json!({"boards": feeds::named_lists(&b, kind, now)});
            json_ok(&json!({
                "updatedAt": now,
                "ready": section(ListKind::ReadyForDev),
                "progress": section(ListKind::InProgress),
            }))
        }
        Err(e) => json_ok(&json!({"error": e.to_string()})),
    }
}

async fn search(
    State(dash): State<Arc<Dash>>,
    h: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !dash.is_ours(&h) {
        return http_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let get = |k: &str| q.get(k).map_or("", String::as_str);
    if let Err(e) = feeds::classify(get("q")) {
        return http_error(StatusCode::BAD_REQUEST, &e.to_string());
    }
    let server = match &dash.trello {
        Ok(server) => server,
        Err(e) => return json_ok(&json!({"error": e})),
    };
    // A total failure must not read as "no matches".
    let boards = match scope(&dash).await {
        Ok(b) => b,
        Err(e) => return json_ok(&e),
    };
    match server
        .search_cards(get("q"), &ids(&boards), get("board"), now() as i64)
        .await
    {
        Ok(cards) => json_ok(&json!({"cards": cards})),
        Err(e) => json_ok(&json!({"error": e.to_string()})),
    }
}

async fn orca_repos(State(dash): State<Arc<Dash>>, h: HeaderMap) -> Response {
    if !dash.is_ours(&h) {
        return http_error(StatusCode::FORBIDDEN, "forbidden");
    }
    match orca::repos(&dash.orca).await {
        Ok(repos) => json_ok(&json!({"repos": repos})),
        Err(e) => json_ok(&json!({"error": e})),
    }
}

/// The cards an agent is on in Orca, so the page shows them and not Send to Orca.
async fn orca_sessions(State(dash): State<Arc<Dash>>, h: HeaderMap) -> Response {
    if !dash.is_ours(&h) {
        return http_error(StatusCode::FORBIDDEN, "forbidden");
    }
    match orca::sessions(&dash.orca).await {
        Ok(sessions) => json_ok(&json!({"sessions": sessions})),
        Err(e) => json_ok(&json!({"error": e})),
    }
}

/// A card as Orca names it: its short link and `#<number> <name>`.
fn card_ref(card: &Value) -> (String, String) {
    (
        mcp::nested_string(card, &["shortLink"]),
        format!(
            "#{} {}",
            card["idShort"],
            mcp::nested_string(card, &["name"])
        ),
    )
}

/// The open card behind `url`, on a board in `boards`.
async fn scoped_card(
    server: &mcp::Server,
    boards: &[Value],
    url: &str,
) -> Result<(String, String), Box<Response>> {
    let Ok(feeds::Query::Link(link)) = feeds::classify(url) else {
        return Err(Box::new(http_error(
            StatusCode::BAD_REQUEST,
            "card must be a Trello card link",
        )));
    };
    match server.card_in_context(&link).await {
        Ok(Some(c)) if ids(boards).contains(&mcp::nested_string(&c, &["idBoard"])) => {
            Ok(card_ref(&c))
        }
        Ok(_) => Err(Box::new(http_error(StatusCode::NOT_FOUND, "no such card"))),
        Err(e) => Err(Box::new(json_ok(&json!({"error": e.to_string()})))),
    }
}

/// Send to Orca (?card=<card link>&repo=<Orca repo id>&agent=claude|codex|agy): starts an agent
/// on the card in its own worktree. Send selected (?cards=<link>,<link>…&repo&agent): the same
/// for a lone card; several go to a planner that groups them and sends each group. Only our page,
/// only open cards on boards in scope.
async fn orca_send(
    State(dash): State<Arc<Dash>>,
    h: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !dash.is_ours(&h) {
        return http_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let get = |k: &str| q.get(k).map_or("", String::as_str);
    let urls: Vec<&str> = match q.get("cards") {
        Some(cards) => cards.split(',').filter(|c| !c.is_empty()).collect(),
        None => vec![get("card")],
    };
    if urls.is_empty() {
        return http_error(StatusCode::BAD_REQUEST, "pick a card");
    }
    if urls
        .iter()
        .any(|u| !matches!(feeds::classify(u), Ok(feeds::Query::Link(_))))
    {
        return http_error(StatusCode::BAD_REQUEST, "card must be a Trello card link");
    }
    if !orca::AGENTS.iter().any(|(a, _)| *a == get("agent")) {
        return http_error(
            StatusCode::BAD_REQUEST,
            "agent must be claude, codex or agy",
        );
    }
    let server = match &dash.trello {
        Ok(server) => server,
        Err(e) => return json_ok(&json!({"error": e})),
    };
    let boards = match scope(&dash).await {
        Ok(b) => b,
        Err(e) => return json_ok(&e),
    };
    let mut cards = Vec::new();
    for url in urls {
        match scoped_card(server, &boards, url).await {
            Ok(c) => cards.push(c),
            Err(r) => return *r,
        }
    }
    let _one = dash.sending.lock().await;
    let sent = if q.contains_key("cards") {
        orca::send_selected(&dash.orca, &dash.cli, get("repo"), get("agent"), &cards).await
    } else {
        orca::send(&dash.orca, get("repo"), get("agent"), &cards).await
    };
    match sent {
        Ok(v) => json_ok(&v),
        Err(e) => json_ok(&json!({"error": e})),
    }
}

/// `trello-mcp orca-send`, what the planner runs for each group: Send to Orca for `cards` (links,
/// in the order to build them), within the MCP server's workspace allow-list.
pub(crate) async fn send_cards(
    config_path: &str,
    repo: &str,
    agent: &str,
    cards: &[String],
) -> Result<Value> {
    let server = mcp::Server::new(Arc::new(trello::Client::new(Arc::new(
        config::Manager::new(config_path)?,
    ))));
    let mut group = Vec::new();
    for url in cards {
        let Ok(feeds::Query::Link(link)) = feeds::classify(url) else {
            bail!("{url}: not a Trello card link");
        };
        server.ensure_card_allowed(&link).await?;
        let card = server
            .card_in_context(&link)
            .await?
            .ok_or_else(|| anyhow!("{url}: no such open card"))?;
        group.push(card_ref(&card));
    }
    orca::send(&orca::cli(), repo, agent, &group)
        .await
        .map_err(|e| anyhow!(e))
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
        start_with_orca(trello, tailscale, "/x/no-orca").await
    }

    async fn start_with_orca(
        trello: Result<mcp::Server, String>,
        tailscale: &str,
        orca: &str,
    ) -> (String, String) {
        let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = local.local_addr().unwrap().port();
        let dash = Dash::new(trello, "/x/trello-mcp".into(), orca.into(), port, tailscale);
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
        let (_, raw) = call(to, "GET", path, &[("Host", to), ("X-Trello-MCP", "1")]).await;
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
        assert_eq!(
            call(&us, "GET", &long, &[("Host", &us), ("X-Trello-MCP", "1")])
                .await
                .0,
            400
        );
        assert_eq!(
            get_json(&us, "/api/search?q=x").await,
            json!({"error": "read config: api_key is required"})
        );
    }

    /// Trello with two boards in scope, b1 (feeds::tests::board) and b2, and a mention on c1.
    async fn two_boards() -> crate::mcp::test_support::Harness {
        test_server(&[], |req| match req.url.path() {
            "/members/me/boards" => json_response(json!([
                {"id": "b1", "name": "Client Portal", "idOrganization": "w1"},
                {"id": "b2", "name": "Gone Board", "idOrganization": "w1"}
            ])),
            "/boards/b1" => json_response(feeds::tests::board()),
            "/members/me/notifications" => {
                json_response(json!([feeds::tests::mention("a1", "c1", 60, "@me hi")]))
            }
            "/members/me/actions" => json_response(json!([])),
            "/cards/c1" | "/cards/aaaa1111" => json_response(feeds::tests::in_context("c1")),
            "/cards/bbbb2222" => json_response(feeds::tests::in_context("c2")),
            _ => not_found(),
        })
        .await
    }

    #[tokio::test]
    async fn trello_view_loads_no_board_until_one_is_picked() {
        let h = two_boards().await;
        let (us, _) = start(Ok(h.server.clone()), "").await;
        let v = get_json(&us, "/api/trello").await;
        assert_eq!(
            v["boards"],
            json!([{"id": "b1", "name": "Client Portal"}, {"id": "b2", "name": "Gone Board"}])
        );
        assert!(v["updatedAt"].as_i64().unwrap() > 1_700_000_000);
        assert_eq!(v["waiting"]["rows"][0]["number"], 348, "{v}");
        assert!(
            !calls(&h.api).await.iter().any(|c| c.contains("/boards/")),
            "no board's cards: {:?}",
            calls(&h.api).await
        );
        let n = calls(&h.api).await.len();
        get_json(&us, "/api/trello").await;
        assert_eq!(
            calls(&h.api).await.len(),
            n,
            "a second load within a minute comes from the cache"
        );
        get_json(&us, "/api/trello?fresh=1").await;
        assert!(calls(&h.api).await.len() > n, "Refresh refetches");

        let b = get_json(&us, "/api/board?id=b1").await;
        assert_eq!(b["progress"]["boards"][0]["cards"][0]["number"], 348, "{b}");
        assert_eq!(
            b["ready"]["boards"][0]["cards"].as_array().unwrap().len(),
            2
        );
        assert_eq!(
            calls(&h.api)
                .await
                .iter()
                .filter(|c| c.contains("/boards/"))
                .collect::<Vec<_>>(),
            ["GET /boards/b1"],
            "only the picked board"
        );
        let hdrs = [("Host", us.as_str()), ("X-Trello-MCP", "1")];
        assert_eq!(
            call(&us, "GET", "/api/board?id=elsewhere", &hdrs).await.0,
            404
        );
        assert!(get_json(&us, "/api/board?id=b2").await["error"].is_string());

        // A card number looks in the picked project only.
        let found = get_json(&us, "/api/search?q=%23348&board=b1").await;
        assert_eq!(found["cards"][0]["name"], "Block physical address");
        let unpicked = get_json(&us, "/api/search?q=%23348").await;
        assert!(
            unpicked["error"]
                .as_str()
                .unwrap()
                .contains("Pick a project")
        );
    }

    const SEND: &str =
        "/api/orca/send?card=https%3A%2F%2Ftrello.com%2Fc%2Faaaa1111&repo=r1&agent=codex";

    /// The headers our page sends to `us`, with its Origin.
    fn ours(us: &str, origin: &str) -> [(&'static str, String); 3] {
        [
            ("Host", us.to_string()),
            ("X-Trello-MCP", "1".to_string()),
            ("Origin", origin.to_string()),
        ]
    }

    #[tokio::test]
    async fn send_to_orca_needs_our_page_a_card_in_scope_and_a_known_agent() {
        let h = two_boards().await;
        // Every refusal comes before Orca: "/x/no-orca" never runs.
        let (us, _) = start(Ok(h.server.clone()), "").await;
        let hdrs = ours(&us, &format!("http://{us}"));
        let mine: Vec<(&str, &str)> = hdrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(call(&us, "POST", SEND, &mine[..1]).await.0, 403);
        assert_eq!(
            call(
                &us,
                "POST",
                SEND,
                &[mine[0], mine[1], ("Origin", "https://evil.example")]
            )
            .await
            .0,
            403
        );
        assert_eq!(call(&us, "GET", SEND, &mine).await.0, 405);
        assert_eq!(
            call(&us, "POST", &SEND.replace("codex", "sh"), &mine)
                .await
                .0,
            400
        );
        assert_eq!(
            call(
                &us,
                "POST",
                "/api/orca/send?card=hello%20world&repo=r1&agent=codex",
                &mine
            )
            .await
            .0,
            400
        );
        assert_eq!(
            call(&us, "POST", &SEND.replace("aaaa1111", "zzzz9999"), &mine)
                .await
                .0,
            404
        );
        // Send selected checks every card before Orca.
        let picked = "/api/orca/send?repo=r1&agent=codex&cards=";
        let aaaa = "https%3A%2F%2Ftrello.com%2Fc%2Faaaa1111";
        for (cards, status) in [
            (String::new(), 400),
            (format!("{aaaa},hello"), 400),
            (
                format!("{aaaa},https%3A%2F%2Ftrello.com%2Fc%2Fzzzz9999"),
                404,
            ),
        ] {
            assert_eq!(
                call(&us, "POST", &format!("{picked}{cards}"), &mine)
                    .await
                    .0,
                status,
                "{cards}"
            );
        }
        for path in ["/api/orca/repos", "/api/orca/sessions"] {
            assert_eq!(call(&us, "GET", path, &mine[..1]).await.0, 403, "{path}");
        }
    }

    #[tokio::test]
    #[ignore = "starts processes, which macOS can freeze after the procs scan test launches a fresh binary: cargo test -- --ignored orca"]
    async fn send_to_orca_starts_the_agent_in_the_cards_worktree() {
        let h = two_boards().await;
        let dir = tempfile::tempdir().unwrap();
        let bin = orca::tests::fake_orca(dir.path());
        let (us, _) = start_with_orca(Ok(h.server.clone()), "", &bin).await;
        let hdrs = ours(&us, &format!("http://{us}"));
        let mine: Vec<(&str, &str)> = hdrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let (status, raw) = call(&us, "POST", SEND, &mine).await;
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(body(&raw)).unwrap();
        assert_eq!(
            (&v["name"], &v["base"], &v["existing"]),
            (
                &json!("trello-aaaa1111"),
                &json!("feature-x"),
                &json!(false)
            ),
            "{v}"
        );
        let calls = orca::tests::logged(dir.path());
        let set = calls.iter().find(|c| c[1] == "set").unwrap();
        assert!(
            set.contains(&"#348 Block physical address".to_string()),
            "{set:?}"
        );
        let repos = get_json(&us, "/api/orca/repos").await;
        assert_eq!(repos["repos"].as_array().unwrap().len(), 1, "{repos}");
        let sessions = get_json(&us, "/api/orca/sessions").await;
        assert_eq!(sessions, json!({"sessions": []}));
        let picked = "/api/orca/send?repo=r1&agent=codex&cards=https%3A%2F%2Ftrello.com%2Fc%2Faaaa1111,https%3A%2F%2Ftrello.com%2Fc%2Fbbbb2222";
        let (status, raw) = call(&us, "POST", picked, &mine).await;
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(body(&raw)).unwrap();
        assert_eq!(v, json!({"planned": true, "cards": 2, "repo": "app"}));
        let calls = orca::tests::logged(dir.path());
        let term = calls
            .iter()
            .find(|c| c[..2] == ["terminal", "create"])
            .unwrap();
        assert!(
            term.iter()
                .any(|a| a.contains("/x/trello-mcp orca-send --repo r1 --agent codex")),
            "{term:?}"
        );
    }

    #[tokio::test]
    async fn trello_routes_need_our_page() {
        let h = test_server(&[], |_| not_found()).await;
        let (us, _) = start(Ok(h.server.clone()), "").await;
        for path in [
            "/api/trello",
            "/api/trello?fresh=1",
            "/api/search?q=x",
            "/api/board?id=b1",
        ] {
            assert_eq!(
                call(&us, "GET", path, &[("Host", &us)]).await.0,
                403,
                "{path}"
            );
        }
        assert!(calls(&h.api).await.is_empty());
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

    #[test]
    fn page_never_injects_html() {
        let page = std::str::from_utf8(PAGE).unwrap();
        for sink in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
        ] {
            assert!(
                !page.contains(sink),
                "{sink}: Trello text must only ever be set as text"
            );
        }
        for wired in [
            "/api/servers",
            "/api/trello",
            "/api/board",
            "/api/search",
            "/api/orca/repos",
            "/api/orca/sessions",
            "/api/orca/send",
            "X-Trello-MCP",
            "open-groups",
            "localStorage",
        ] {
            assert!(page.contains(wired), "{wired}");
        }
    }
}
