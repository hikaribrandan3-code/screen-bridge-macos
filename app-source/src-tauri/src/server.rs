//! Local HTTP + WebSocket server. The tablet opens `/` in any browser and
//! receives the display as a stream of JPEG frames over `/stream` —
//! zero-install on iPad, Android, or anything with a browser.
//!
//! The virtual display isn't created until the first client's `hello`
//! message reports its real screen size — before that we don't know what
//! resolution to create it at, and creating one eagerly at some arbitrary
//! fixed size is what caused the mismatched-aspect-ratio / tiny-UI bugs.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Form, Query, State,
    },
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use futures_util::{stream::SplitStream, SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;

use crate::capture::{self, Frame};
use crate::virtual_display::VirtualDisplay;

const MIN_DIM: u32 = 640;
const MAX_DIM: u32 = 2560;

const TOUCH_ICON: &[u8] = include_bytes!("../icons/ios/AppIcon-60x60@3x.png");

/// Shared between the Tauri command that owns session lifecycle and every
/// WebSocket handler task. Empty until the first client connects and
/// reports its screen size; torn down explicitly on disconnect.
#[derive(Default)]
pub struct DisplaySlot {
    pub display: Option<VirtualDisplay>,
    pub frames: Option<broadcast::Sender<Frame>>,
    pub running: Option<Arc<AtomicBool>>,
}

#[derive(Clone)]
struct ServerState {
    token: String,
    pin: String,
    clients: Arc<AtomicUsize>,
    app: AppHandle,
    slot: Arc<Mutex<DisplaySlot>>,
    quality: (u32, u8),
    pin_guard: Arc<Mutex<PinGuard>>,
    audio_tx: broadcast::Sender<Arc<Vec<u8>>>,
    audio_sample_rate: f64,
    audio_channels: u32,
}

/// PIN pairing is for devices that can't scan the QR (TVs, projectors) —
/// a short code typed manually is a much weaker secret than the 24-char
/// token, so it gets its own protection: locked out after repeated wrong
/// guesses, and dead the instant it's used once successfully.
#[derive(Default)]
struct PinGuard {
    consumed: bool,
    attempts: u32,
    locked_until: Option<Instant>,
}

const PIN_MAX_ATTEMPTS: u32 = 5;
const PIN_LOCKOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct TokenQuery {
    t: Option<String>,
}

#[derive(Deserialize)]
struct PairForm {
    pin: String,
}

#[derive(Deserialize)]
struct Hello {
    #[serde(rename = "type")]
    kind: String,
    vw: u32,
    vh: u32,
}

pub async fn run(
    port: u16,
    token: String,
    pin: String,
    clients: Arc<AtomicUsize>,
    app: AppHandle,
    slot: Arc<Mutex<DisplaySlot>>,
    quality: (u32, u8),
    audio_tx: broadcast::Sender<Arc<Vec<u8>>>,
    audio_sample_rate: f64,
    audio_channels: u32,
) -> Result<(), String> {
    let state = ServerState {
        token,
        pin,
        clients,
        app,
        slot,
        quality,
        pin_guard: Arc::new(Mutex::new(PinGuard::default())),
        audio_tx,
        audio_sample_rate,
        audio_channels,
    };
    let router = Router::new()
        .route("/", get(index))
        .route("/stream", get(ws_upgrade))
        .route("/audio", get(audio_ws_upgrade))
        .route("/manifest.json", get(manifest))
        .route("/apple-touch-icon.png", get(touch_icon))
        .route("/pair", get(pair_page).post(pair_submit))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| format!("could not bind port {port}: {e}"))?;
    axum::serve(listener, router)
        .await
        .map_err(|e| e.to_string())
}

fn token_ok(state: &ServerState, q: &TokenQuery) -> bool {
    q.t.as_deref() == Some(state.token.as_str())
}

async fn index(Query(q): Query<TokenQuery>, State(state): State<ServerState>) -> Response {
    if !token_ok(&state, &q) {
        // No token: redirect to PIN pairing page. Users on TVs/projectors can
        // just type screenbridge.local:47788 and land here, then enter the PIN.
        return Redirect::to("/pair").into_response();
    }
    Html(include_str!("../tablet.html")).into_response()
}

async fn manifest(Query(q): Query<TokenQuery>, State(state): State<ServerState>) -> Response {
    if !token_ok(&state, &q) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    (
        [("content-type", "application/manifest+json")],
        include_str!("../manifest.json"),
    )
        .into_response()
}

async fn touch_icon() -> Response {
    ([("content-type", "image/png")], TOUCH_ICON).into_response()
}

/// Deliberately unauthenticated — this is the entry point for devices that
/// don't have the token yet (typed manually instead of scanned from the QR).
async fn pair_page() -> Response {
    Html(include_str!("../pair.html")).into_response()
}

async fn pair_submit(State(state): State<ServerState>, Form(form): Form<PairForm>) -> Response {
    let mut guard = state.pin_guard.lock().unwrap();

    if let Some(until) = guard.locked_until {
        if Instant::now() < until {
            return Redirect::to("/pair?err=locked").into_response();
        }
        guard.attempts = 0;
        guard.locked_until = None;
    }

    if guard.consumed || form.pin.trim() != state.pin {
        guard.attempts += 1;
        if guard.attempts >= PIN_MAX_ATTEMPTS {
            guard.locked_until = Some(Instant::now() + PIN_LOCKOUT);
            return Redirect::to("/pair?err=locked").into_response();
        }
        return Redirect::to("/pair?err=wrong").into_response();
    }

    guard.consumed = true;
    Redirect::to(&format!("/?t={}", state.token)).into_response()
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    Query(q): Query<TokenQuery>,
    State(state): State<ServerState>,
) -> Response {
    if !token_ok(&state, &q) {
        return (StatusCode::UNAUTHORIZED, "invalid pairing token").into_response();
    }
    ws.on_upgrade(move |socket| handle_client(socket, state))
        .into_response()
}

async fn audio_ws_upgrade(
    ws: WebSocketUpgrade,
    Query(q): Query<TokenQuery>,
    State(state): State<ServerState>,
) -> Response {
    if !token_ok(&state, &q) {
        return (StatusCode::UNAUTHORIZED, "invalid pairing token").into_response();
    }
    ws.on_upgrade(move |socket| handle_audio_client(socket, state))
        .into_response()
}

/// Send-only — raw Float32 PCM chunks, no input relay on this socket. First
/// message is the sample rate as JSON text so the client can size its
/// AudioContext correctly instead of guessing.
async fn handle_audio_client(mut socket: WebSocket, state: ServerState) {
    let hello = serde_json::json!({
        "sampleRate": state.audio_sample_rate,
        "channels": state.audio_channels,
    })
    .to_string();
    if socket.send(Message::Text(hello)).await.is_err() {
        return;
    }

    let mut rx = state.audio_tx.subscribe();
    loop {
        match rx.recv().await {
            Ok(chunk) => {
                if socket.send(Message::Binary((*chunk).clone())).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn handle_client(socket: WebSocket, state: ServerState) {
    let n = state.clients.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = state.app.emit("clients", n);

    let (mut sink, mut stream) = socket.split();

    // A display from an earlier client already exists — join it directly.
    let existing = {
        let slot = state.slot.lock().unwrap();
        match (&slot.display, &slot.frames) {
            (Some(d), Some(tx)) => Some((d.display_id, tx.clone())),
            _ => None,
        }
    };

    let joined = match existing {
        Some(pair) => Some(pair),
        None => wait_for_hello_and_start(&state, &mut stream).await,
    };

    let Some((display_id, frames_tx)) = joined else {
        let n = state.clients.fetch_sub(1, Ordering::SeqCst) - 1;
        let _ = state.app.emit("clients", n);
        return;
    };

    let mut frames_rx = frames_tx.subscribe();

    loop {
        tokio::select! {
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => crate::input::handle_message(display_id, &text),
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            frame = frames_rx.recv() => {
                match frame {
                    Ok(frame) => {
                        if sink
                            .send(Message::Text(String::from_utf8_lossy(&frame.to_json()).to_string()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                        if let Some(jpeg) = frame.jpeg {
                            if sink.send(Message::Binary(jpeg)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    let n = state.clients.fetch_sub(1, Ordering::SeqCst) - 1;
    let _ = state.app.emit("clients", n);
}

/// Blocks (within this client's task only — other clients are unaffected)
/// until a `hello` arrives, then creates the virtual display at the
/// reported size and starts capture. Returns `None` if the socket closes
/// first.
async fn wait_for_hello_and_start(
    state: &ServerState,
    stream: &mut SplitStream<WebSocket>,
) -> Option<(u32, broadcast::Sender<Frame>)> {
    while let Some(Ok(msg)) = stream.next().await {
        if let Message::Text(text) = msg {
            if let Some(pair) = try_start_display(state, &text) {
                return Some(pair);
            }
        }
    }
    None
}

fn try_start_display(state: &ServerState, text: &str) -> Option<(u32, broadcast::Sender<Frame>)> {
    let hello: Hello = serde_json::from_str(text).ok()?;
    if hello.kind != "hello" {
        return None;
    }

    let mut slot = state.slot.lock().unwrap();
    // Another client's hello may have already created it while we waited.
    if let (Some(d), Some(tx)) = (&slot.display, &slot.frames) {
        return Some((d.display_id, tx.clone()));
    }

    let w = hello.vw.clamp(MIN_DIM, MAX_DIM);
    let h = hello.vh.clamp(MIN_DIM, MAX_DIM);
    let display = VirtualDisplay::create("Screen Bridge", w, h).ok()?;
    let display_id = display.display_id;

    let (tx, _) = broadcast::channel::<Frame>(4);
    let running = Arc::new(AtomicBool::new(true));
    capture::start(
        display_id,
        tx.clone(),
        running.clone(),
        state.quality.0,
        state.quality.1,
    );

    slot.display = Some(display);
    slot.frames = Some(tx.clone());
    slot.running = Some(running);
    let _ = state.app.emit("streaming-started", display_id);

    Some((display_id, tx))
}
