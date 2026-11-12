//! Embedded HTTP + WebSocket server: serves the web UI (assets compiled
//! into the binary) and exposes the command bus to remote clients.

use crate::runtime::{parse_command, redact_state, ClientInfo, Handle, Outbound, Role, VERSION};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast::error::RecvError;

#[derive(rust_embed::RustEmbed)]
#[folder = "$OUT_DIR/ui"]
struct Assets;

pub struct AppState {
    handle: Handle,
    admin_token: String,
    next_client: AtomicU64,
}

type App = Arc<AppState>;

pub(crate) fn random_token() -> String {
    let mut buf = [0u8; 16];
    if getrandom::fill(&mut buf).is_err() {
        // Extremely unlikely; fall back to time-derived bits rather than failing.
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        buf.copy_from_slice(&t.to_le_bytes());
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Non-loopback IPv4 addresses of this machine (for URL display / QR codes).
/// `http://host:port`, without the port when it is the standard web port 80.
pub fn base_url(host: &str, port: u16) -> String {
    if port == 80 {
        format!("http://{host}")
    } else {
        format!("http://{host}:{port}")
    }
}

pub fn local_urls(port: u16) -> Vec<String> {
    let mut urls: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            IpAddr::V4(ip) => Some(base_url(&ip.to_string(), port)),
            IpAddr::V6(_) => None,
        })
        .collect();
    urls.sort();
    urls.dedup();
    urls
}

pub fn router(handle: Handle) -> Router {
    let state: App = Arc::new(AppState {
        handle,
        admin_token: random_token(),
        next_client: AtomicU64::new(1),
    });
    Router::new()
        .route("/api/session", get(session))
        .route("/api/login", post(login))
        .route("/ws", get(ws_upgrade))
        .route("/api/v1/state", get(api_state))
        .route("/api/v1/command", post(api_command))
        .route("/api/v1/cmd/{name}", any(api_cmd))
        .route("/api/fs", get(api_fs))
        .route("/api/waveform", get(api_waveform))
        .route("/api/qr.svg", get(api_qr))
        .route("/api/cuesheet.pdf", get(api_cuesheet_pdf))
        .route(
            "/api/link",
            post(api_link).layer(axum::extract::DefaultBodyLimit::max(64 << 20)),
        )
        .route("/api/cuesheet.csv", get(api_cuesheet_csv))
        .route("/api/diagnostics", get(api_diagnostics))
        .fallback(static_asset)
        .with_state(state)
}

/// Serve until the listener fails or the task is aborted.
pub async fn serve(handle: Handle, listener: tokio::net::TcpListener) -> std::io::Result<()> {
    serve_all(handle, vec![listener]).await
}

/// Serve the same application (shared logins) on several listeners, e.g. the
/// configured port and the standard web port 80. Returns when the first
/// listener fails.
pub async fn serve_all(
    handle: Handle,
    listeners: Vec<tokio::net::TcpListener>,
) -> std::io::Result<()> {
    let app = router(handle);
    let mut tasks = tokio::task::JoinSet::new();
    for listener in listeners {
        let service = app
            .clone()
            .into_make_service_with_connect_info::<SocketAddr>();
        tasks.spawn(async move { axum::serve(listener, service).await });
    }
    match tasks.join_next().await {
        Some(Ok(result)) => result,
        Some(Err(e)) => Err(std::io::Error::other(e)),
        None => Ok(()),
    }
}

fn pins(app: &AppState) -> (String, String) {
    let state = app.handle.state();
    let remote = &state["show"]["settings"]["remote"];
    (
        remote["control_pin"].as_str().unwrap_or("").to_string(),
        remote["view_pin"].as_str().unwrap_or("").to_string(),
    )
}

/// Token comparison without early exit.
pub(crate) fn token_eq(a: &str, b: &str) -> bool {
    pin_eq(a, b)
}

/// Constant-time-ish comparison; PINs are short, this just avoids early exit.
fn pin_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn role_for(app: &AppState, addr: &SocketAddr, token: Option<&str>) -> Option<Role> {
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        if token == app.admin_token {
            return Some(Role::Admin);
        }
        if let Some(pin) = token.strip_prefix("pin:") {
            let (control, view) = pins(app);
            if !control.is_empty() && pin_eq(pin, &control) {
                return Some(Role::Control);
            }
            if pin_eq(pin, &view) {
                return Some(Role::View);
            }
            return None;
        }
        if let Some(role) = app
            .handle
            .sessions
            .lock()
            .ok()
            .and_then(|t| t.get(token).copied())
        {
            return Some(role);
        }
        // A paired device (QR code with token).
        return app
            .handle
            .pairings
            .lock()
            .ok()
            .and_then(|p| p.role_of(token));
    }
    // The machine running the show is trusted: its own UI needs no login.
    addr.ip().is_loopback().then_some(Role::Admin)
}

fn token_of<'a>(query: &'a HashMap<String, String>, headers: &'a HeaderMap) -> Option<&'a str> {
    query.get("token").map(String::as_str).or_else(|| {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
    })
}

fn deny() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "ok": false, "error": "forbidden" })),
    )
        .into_response()
}

async fn session(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Json<Value> {
    let (_, view_pin) = pins(&app);
    let view_needs_pin = !view_pin.is_empty();
    // A client that already holds a valid token gets it confirmed.
    if let Some(token) = token_of(&q, &headers).filter(|t| !t.is_empty()) {
        if let Some(role) = role_for(&app, &addr, Some(token)) {
            return Json(
                json!({ "role": role, "token": token, "view_needs_pin": view_needs_pin, "version": VERSION }),
            );
        }
    }
    if addr.ip().is_loopback() {
        Json(
            json!({ "role": "admin", "token": app.admin_token, "view_needs_pin": view_needs_pin, "version": VERSION }),
        )
    } else {
        Json(
            json!({ "role": null, "token": null, "view_needs_pin": view_needs_pin, "version": VERSION }),
        )
    }
}

#[derive(Deserialize)]
struct Login {
    #[serde(default)]
    pin: String,
    #[serde(default)]
    role: Option<Role>,
}

async fn login(State(app): State<App>, Json(body): Json<Login>) -> Response {
    let (control, view) = pins(&app);
    let wanted = body.role.unwrap_or(Role::Control);
    let role = match wanted {
        Role::View if pin_eq(&body.pin, &view) => Some(Role::View),
        // The control PIN also opens the view page.
        Role::View | Role::Control | Role::Admin
            if !control.is_empty() && pin_eq(&body.pin, &control) =>
        {
            Some(if wanted == Role::View {
                Role::View
            } else {
                Role::Control
            })
        }
        _ => None,
    };
    match role {
        Some(role) => {
            let token = random_token();
            if let Ok(mut t) = app.handle.sessions.lock() {
                t.insert(token.clone(), role);
            }
            Json(json!({ "token": token, "role": role })).into_response()
        }
        None => {
            // Slow down PIN guessing.
            tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "wrong PIN" })),
            )
                .into_response()
        }
    }
}

async fn api_state(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(role) = role_for(&app, &addr, token_of(&q, &headers)) else {
        return deny();
    };
    let mut state = app.handle.state();
    if role < Role::Admin {
        state = redact_state(state);
    }
    if let Some(obj) = state.as_object_mut() {
        obj.insert("tick".into(), app.handle.tick());
    }
    Json(state).into_response()
}

async fn run_command(app: &AppState, role: Role, origin: &str, cmd: Value) -> Response {
    let result = match parse_command(cmd) {
        Ok(c) => app.handle.command(c, role, origin).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(v) => Json(json!({ "ok": true, "result": v })).into_response(),
        Err(e) => {
            let code = if e == "forbidden" {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::BAD_REQUEST
            };
            (code, Json(json!({ "ok": false, "error": e }))).into_response()
        }
    }
}

async fn api_command(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
    Json(cmd): Json<Value>,
) -> Response {
    let Some(role) = role_for(&app, &addr, token_of(&q, &headers)) else {
        return deny();
    };
    run_command(&app, role, &format!("http {}", addr.ip()), cmd).await
}

/// `/api/v1/cmd/go`, `/api/v1/cmd/goto_number?number=14`: query parameters
/// become command fields (numbers, booleans and null are recognised).
async fn api_cmd(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    UrlPath(name): UrlPath<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(role) = role_for(&app, &addr, token_of(&q, &headers)) else {
        return deny();
    };
    let mut cmd = Map::new();
    for (k, v) in &q {
        if k == "token" {
            continue;
        }
        // Cue numbers and timecodes stay strings; ids and levels become numbers.
        let keep_text = matches!(
            k.as_str(),
            "number" | "to" | "from" | "text" | "path" | "addr"
        );
        let value = if keep_text {
            Value::from(v.as_str())
        } else {
            serde_json::from_str::<Value>(v)
                .ok()
                .filter(|p| !p.is_object() && !p.is_array())
                .unwrap_or_else(|| Value::from(v.as_str()))
        };
        cmd.insert(k.clone(), value);
    }
    cmd.insert("cmd".into(), Value::from(name));
    run_command(
        &app,
        role,
        &format!("http {}", addr.ip()),
        Value::Object(cmd),
    )
    .await
}

const MEDIA_EXT: [&str; 12] = [
    "wav", "wave", "aif", "aiff", "flac", "mp3", "ogg", "oga", "m4a", "aac", "caf", "smshow",
];

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

async fn api_fs(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if role_for(&app, &addr, token_of(&q, &headers)) != Some(Role::Admin) {
        return deny();
    }
    let dir = match q.get("path").filter(|p| !p.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => app.handle.state()["show_path"]
            .as_str()
            .and_then(|p| Path::new(p).parent().map(Path::to_path_buf))
            .unwrap_or_else(home_dir),
    };
    let listing = tokio::task::spawn_blocking(move || {
        let mut entries = Vec::new();
        for e in std::fs::read_dir(&dir)?.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            let is_dir = meta.is_dir();
            let ext = Path::new(&name).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
            if is_dir || MEDIA_EXT.contains(&ext.as_str()) {
                entries.push((is_dir, name, e.path(), meta.len()));
            }
        }
        entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
        let list: Vec<Value> = entries.into_iter().map(|(dir, name, path, size)| json!({ "name": name, "path": path, "dir": dir, "size": size })).collect();
        Ok::<_, std::io::Error>(json!({ "path": dir, "parent": dir.parent(), "entries": list }))
    })
    .await;
    match listing {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn api_waveform(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if role_for(&app, &addr, token_of(&q, &headers)) != Some(Role::Admin) {
        return deny();
    }
    let Some(file) = q.get("file").filter(|f| !f.is_empty()).cloned() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing file" })),
        )
            .into_response();
    };
    let buckets = q
        .get("buckets")
        .and_then(|b| b.parse::<usize>().ok())
        .unwrap_or(800)
        .clamp(16, 8000);
    let show_dir = app.handle.state()["show_path"]
        .as_str()
        .and_then(|p| Path::new(p).parent().map(Path::to_path_buf));
    let result = tokio::task::spawn_blocking(move || {
        let path = sm_core::showfile::resolve_media(show_dir.as_deref(), &file);
        let info = sm_audio::probe(&path).map_err(|e| e.to_string())?;
        let peaks = sm_audio::waveform_peaks(&path, buckets).map_err(|e| e.to_string())?;
        let peaks: Vec<[f32; 2]> = peaks.into_iter().map(|(lo, hi)| [lo, hi]).collect();
        Ok::<_, String>(json!({ "duration": info.duration_seconds, "channels": info.channels, "sample_rate": info.sample_rate, "peaks": peaks }))
    })
    .await;
    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Messages from the main computer (backup role, shared key).
async fn api_link(State(app): State<App>, headers: HeaderMap, Json(msg): Json<Value>) -> Response {
    let key = headers
        .get(crate::link::HEADER)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let (ok, taken_over) = app
        .handle
        .link
        .lock()
        .map(|l| (l.accepts(key), l.taken_over()))
        .unwrap_or((false, false));
    if !ok {
        return deny();
    }
    if taken_over {
        // Running on its own now: the (returned) main must not steer it.
        return (
            StatusCode::CONFLICT,
            Json(json!({ "ok": false, "error": "taken_over" })),
        )
            .into_response();
    }
    let cmd = crate::runtime::AnyCommand::Sys(crate::runtime::SysCommand::LinkApply { msg });
    match app.handle.command(cmd, Role::Admin, "link").await {
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": e })),
        )
            .into_response(),
    }
}

/// The cue sheet, for any role that may see the cue list (`view` and up).
fn cuesheet(
    app: &AppState,
    addr: &SocketAddr,
    q: &HashMap<String, String>,
    headers: &HeaderMap,
    pdf: bool,
) -> Response {
    if role_for(app, addr, token_of(q, headers)).is_none() {
        return deny();
    }
    let state = app.handle.state();
    let Ok(show) = serde_json::from_value::<sm_core::model::Show>(state["show"].clone()) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "no show").into_response();
    };
    let (ctype, body) = if pdf {
        let date = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        ("application/pdf", sm_core::cuesheet::to_pdf(&show, &date))
    } else {
        (
            "text/csv; charset=utf-8",
            sm_core::cuesheet::to_csv(&show).into_bytes(),
        )
    };
    let name = if pdf {
        "cue-sheet.pdf"
    } else {
        "cue-sheet.csv"
    };
    (
        [
            (header::CONTENT_TYPE, ctype.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("inline; filename=\"{name}\""),
            ),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        body,
    )
        .into_response()
}

async fn api_cuesheet_pdf(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    cuesheet(&app, &addr, &q, &headers, true)
}

async fn api_cuesheet_csv(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    cuesheet(&app, &addr, &q, &headers, false)
}

async fn api_qr(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if role_for(&app, &addr, token_of(&q, &headers)) != Some(Role::Admin) {
        return deny();
    }
    let state = app.handle.state();
    let index = q
        .get("index")
        .and_then(|i| i.parse::<usize>().ok())
        .unwrap_or(0);
    let Some(base) = state["remote"]["urls"].get(index).and_then(Value::as_str) else {
        return (StatusCode::NOT_FOUND, "no network address").into_response();
    };
    let page = if q.get("role").map(String::as_str) == Some("view") {
        "view"
    } else {
        "remote"
    };
    let url = format!("{base}/{page}");
    match qrcode::QrCode::new(url.as_bytes()) {
        Ok(code) => {
            let svg = code
                .render::<qrcode::render::svg::Color<'_>>()
                .min_dimensions(240, 240)
                .quiet_zone(true)
                .build();
            (
                [
                    (header::CONTENT_TYPE, "image/svg+xml"),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                svg,
            )
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_diagnostics(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if role_for(&app, &addr, token_of(&q, &headers)) != Some(Role::Admin) {
        return deny();
    }
    let out = app.handle.diagnostics();
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"showmaster-diagnostics.txt\"",
            ),
        ],
        out,
    )
        .into_response()
}

async fn static_asset(ConnectInfo(addr): ConnectInfo<SocketAddr>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    }
    // Tablets and phones only type `showmaster.local`: send them to the
    // read-only view (it has a "Control" button that asks for the control PIN).
    // This computer itself keeps the full interface.
    if path.is_empty() && !addr.ip().is_loopback() {
        return (
            StatusCode::FOUND,
            [
                (header::LOCATION, "/view"),
                (header::CACHE_CONTROL, "no-store"),
            ],
        )
            .into_response();
    }
    let (file, name) = match Assets::get(path) {
        Some(f) if !path.is_empty() => (f, path),
        // Client-side routes (/remote, /view) all load the single-page app.
        _ if !path.contains('.') => match Assets::get("index.html") {
            Some(f) => (f, "index.html"),
            None => return StatusCode::NOT_FOUND.into_response(),
        },
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_ref().to_string()),
            (header::CACHE_CONTROL, cache.to_string()),
        ],
        file.data.into_owned(),
    )
        .into_response()
}

async fn ws_upgrade(
    State(app): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let token = token_of(&q, &headers).map(str::to_string);
    let Some(role) = role_for(&app, &addr, token.as_deref()) else {
        return deny();
    };
    let agent = headers
        .get(header::USER_AGENT)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(120)
        .collect::<String>();
    ws.on_upgrade(move |socket| client(app, socket, addr, role, token, agent))
}

fn state_message(app: &AppState, role: Role) -> (u64, String) {
    let (rev, mut state) = match app.handle.published.read() {
        Ok(p) => (p.rev, p.state.clone()),
        Err(_) => (0, Value::Null),
    };
    if role < Role::Admin {
        state = redact_state(state);
    }
    (
        rev,
        json!({ "t": "state", "rev": rev, "state": state }).to_string(),
    )
}

async fn client(
    app: App,
    socket: WebSocket,
    addr: SocketAddr,
    role: Role,
    token: Option<String>,
    agent: String,
) {
    let id = app.next_client.fetch_add(1, Ordering::Relaxed);
    let since_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if let Ok(mut c) = app.handle.clients.lock() {
        let token = token.clone().unwrap_or_default();
        let device = app
            .handle
            .pairings
            .lock()
            .ok()
            .and_then(|p| p.name_of(&token))
            .unwrap_or_default();
        c.push(ClientInfo {
            id,
            role,
            addr: addr.ip().to_string(),
            since_ms,
            agent,
            device,
            token,
        });
    }
    let origin = format!("ws#{id} {}", addr.ip());
    tracing::info!("client {origin} connected as {role:?}");
    let (mut tx, mut rx) = socket.split();
    // Subscribe before taking the snapshot so no patch can be missed.
    let mut events = app.handle.events.subscribe();
    let hello =
        json!({ "t": "hello", "role": role, "client_id": id, "version": VERSION }).to_string();
    let (mut rev, state) = state_message(&app, role);
    let log_tail = app
        .handle
        .published
        .read()
        .map(|p| p.log_tail.clone())
        .unwrap_or_default();
    let mut ok = tx.send(Message::Text(hello.into())).await.is_ok()
        && tx.send(Message::Text(state.into())).await.is_ok();
    if ok && !log_tail.is_empty() {
        ok = tx
            .send(Message::Text(
                json!({ "t": "log", "entries": log_tail })
                    .to_string()
                    .into(),
            ))
            .await
            .is_ok();
    }
    while ok {
        tokio::select! {
            ev = events.recv() => match ev {
                Ok(Outbound::Patch { rev: r, admin, public }) => {
                    if r <= rev {
                        continue;
                    }
                    rev = r;
                    let text = if role == Role::Admin { admin } else { public };
                    ok = tx.send(Message::Text(text.as_ref().into())).await.is_ok();
                }
                Ok(Outbound::Tick(t)) | Ok(Outbound::Log(t)) => ok = tx.send(Message::Text(t.as_ref().into())).await.is_ok(),
                Ok(Outbound::Kick(target)) => {
                    if target == id {
                        let _ = tx.send(Message::Text(json!({ "t": "kicked" }).to_string().into())).await;
                        // A kicked client must log in again.
                        if let (Some(t), Ok(mut tokens)) = (&token, app.handle.sessions.lock()) {
                            tokens.remove(t);
                        }
                        break;
                    }
                }
                // Too slow to keep up: resynchronise with a full snapshot.
                Err(RecvError::Lagged(_)) => {
                    let (r, state) = state_message(&app, role);
                    rev = r;
                    ok = tx.send(Message::Text(state.into())).await.is_ok();
                }
                Err(RecvError::Closed) => break,
            },
            msg = rx.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                    match v.get("t").and_then(Value::as_str) {
                        Some("ping") => {
                            let pong = json!({ "t": "pong", "ts": v.get("ts") }).to_string();
                            ok = tx.send(Message::Text(pong.into())).await.is_ok();
                        }
                        Some("cmd") => {
                            let cmd_id = v.get("id").cloned().unwrap_or(Value::Null);
                            let result = match parse_command(v.get("cmd").cloned().unwrap_or(Value::Null)) {
                                Ok(c) => app.handle.command(c, role, &origin).await,
                                Err(e) => Err(e),
                            };
                            let ack = match result {
                                Ok(r) => json!({ "t": "ack", "id": cmd_id, "ok": true, "result": r }),
                                Err(e) => json!({ "t": "ack", "id": cmd_id, "ok": false, "error": e }),
                            };
                            ok = tx.send(Message::Text(ack.to_string().into())).await.is_ok();
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    if let Ok(mut c) = app.handle.clients.lock() {
        c.retain(|c| c.id != id);
    }
    tracing::info!("client {origin} disconnected");
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn peer(ip: [u8; 4]) -> ConnectInfo<SocketAddr> {
        ConnectInfo(SocketAddr::from((ip, 50000)))
    }

    #[tokio::test]
    async fn root_sends_network_devices_to_the_view() {
        let r = static_asset(peer([192, 168, 1, 30]), Uri::from_static("/")).await;
        assert_eq!(r.status(), StatusCode::FOUND);
        assert_eq!(r.headers()[header::LOCATION], "/view");
        // This computer keeps the full interface; other pages are not redirected.
        let r = static_asset(peer([127, 0, 0, 1]), Uri::from_static("/")).await;
        assert_ne!(r.status(), StatusCode::FOUND);
        let r = static_asset(peer([192, 168, 1, 30]), Uri::from_static("/remote")).await;
        assert_ne!(r.status(), StatusCode::FOUND);
    }

    #[test]
    fn base_urls() {
        assert_eq!(base_url("192.168.1.2", 80), "http://192.168.1.2");
        assert_eq!(base_url("192.168.1.2", 7400), "http://192.168.1.2:7400");
    }
}
