#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio_out;
mod capture;
mod discovery;
mod input;
mod server;
mod virtual_display;

use discovery::{Device, Discovery};
use mdns_sd::ServiceDaemon;
use qrcode::render::svg;
use qrcode::QrCode;
use rand::Rng;
use serde::Serialize;
use server::DisplaySlot;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};

const PORT: u16 = 47788;

struct Session {
    slot: Arc<Mutex<DisplaySlot>>,
    server: tauri::async_runtime::JoinHandle<()>,
    url: String,
    clients: Arc<AtomicUsize>,
    _mdns: Option<ServiceDaemon>,
    // Held for the session's lifetime so the tap/aggregate device/IOProc stay
    // alive; dropping this tears audio capture down. `None` if audio capture
    // failed to start (e.g. permission not yet granted) — video/input still
    // work fine without it, this is a graceful degradation, not a hard error.
    _audio: Option<audio_out::AudioCapture>,
}

struct AppState {
    discovery: Discovery,
    session: Mutex<Option<Session>>,
}

#[derive(Serialize)]
struct ConnectInfo {
    url: String,
    qr_svg: String,
    pin: String,
    /// False when system-audio capture couldn't start (permission not yet
    /// granted, or audio routing set to Mac-only) — the UI shows this instead
    /// of letting audio fail silently.
    audio_ok: bool,
}

#[derive(Serialize)]
struct Status {
    connected: bool,
    clients: usize,
    url: Option<String>,
}

#[tauri::command]
fn list_devices(state: State<AppState>) -> Vec<Device> {
    state.discovery.list()
}

#[tauri::command]
fn get_status(state: State<AppState>) -> Status {
    let session = state.session.lock().unwrap();
    match session.as_ref() {
        Some(s) => Status {
            connected: true,
            clients: s.clients.load(Ordering::SeqCst),
            url: Some(s.url.clone()),
        },
        None => Status {
            connected: false,
            clients: 0,
            url: None,
        },
    }
}

#[tauri::command]
fn connect_display(
    state: State<AppState>,
    app: tauri::AppHandle,
    mode: String,
    quality: String,
    audio: String,
    resolution: String,
) -> Result<ConnectInfo, String> {
    let mut session = state.session.lock().unwrap();
    if session.is_some() {
        return Err("already connected".into());
    }

    if !virtual_display::ensure_capture_access() {
        return Err("screen recording permission denied".into());
    }

    // The capture loop self-paces to whatever the Mac can sustain (it never
    // sleeps negative), so it's safe to target 60 here — actual throughput
    // depends on CPU and WiFi, this is a ceiling, not a guarantee.
    let quality_pair = match quality.as_str() {
        "low" => (20, 40),
        "high" => (60, 60),
        _ => (30, 52),
    };

    let token = generate_token();
    let pin = generate_pin();
    let clients = Arc::new(AtomicUsize::new(0));
    let slot = Arc::new(Mutex::new(DisplaySlot::default()));

    // System audio (Mac -> tablet speaker). Independent of the virtual
    // display's lazy-creation state machine — it doesn't need a tablet's
    // hello, so it starts immediately alongside the server. A failure here
    // (e.g. audio-capture permission not yet granted) doesn't block
    // video/input, which work fine on their own — but it IS surfaced to the
    // UI via `audio_ok` instead of failing silently.
    //
    // Routing: "both" = Mac speakers + tablet; "tablet" = tablet only (Mac
    // muted while streaming, like a real external monitor); "off" = no
    // audio capture at all.
    let (audio_tx, _) = tokio::sync::broadcast::channel::<Arc<Vec<u8>>>(32);
    let (audio, audio_sample_rate, audio_channels) = if audio == "off" {
        (None, 0.0, 0)
    } else {
        match audio_out::start(audio_tx.clone(), audio == "tablet") {
            Ok((capture, rate, channels)) => (Some(capture), rate, channels),
            Err(e) => {
                eprintln!("audio capture unavailable: {e}");
                (None, 0.0, 0)
            }
        }
    };
    let audio_ok = audio.is_some();

    let server_token = token.clone();
    let server_pin = pin.clone();
    let server_clients = clients.clone();
    let server_app = app.clone();
    let server_slot = slot.clone();
    let server_audio_tx = audio_tx.clone();
    // Bind before reporting success so a busy port is visible in the Mac UI.
    let listener = std::net::TcpListener::bind(("0.0.0.0", PORT))
        .map_err(|e| format!("could not bind local server port {PORT}: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| format!("could not start local server: {e}"))?;
    let error_app = app.clone();
    let server = tauri::async_runtime::spawn(async move {
        if let Err(e) = server::run(
            listener,
            server_token,
            server_pin,
            server_clients,
            server_app,
            server_slot,
            quality_pair,
            server_audio_tx,
            audio_sample_rate,
            audio_channels,
        )
        .await
        {
            eprintln!("server error: {e}");
            let _ = error_app.emit("session-error", e);
        }
    });

    let host = pick_host(&mode);
    // "standard" streams at 1x instead of the tablet's Retina scale — 4x
    // fewer pixels to capture/encode/ship, dramatically smoother on WiFi.
    // The tablet page reads this from its own URL, so it costs nothing here.
    let res_param = if resolution == "standard" { "&res=std" } else { "" };
    let url = format!("http://{host}:{PORT}/?t={token}{res_param}");
    let qr_svg = QrCode::new(url.as_bytes())
        .map(|code| {
            code.render::<svg::Color>()
                .min_dimensions(120, 120)
                .quiet_zone(false)
                .build()
        })
        .unwrap_or_default();

    // Advertise screenbridge.local via mDNS so TVs/projectors can find the
    // service without knowing the IP address. The daemon is stored in the
    // session so it persists for the entire session duration.
    let mdns = discovery::advertise_service(PORT);

    *session = Some(Session {
        slot,
        server,
        url: url.clone(),
        clients,
        _mdns: mdns,
        _audio: audio,
    });

    Ok(ConnectInfo { url, qr_svg, pin, audio_ok })
}

#[tauri::command]
fn disconnect_display(state: State<AppState>) {
    let mut session = state.session.lock().unwrap();
    if let Some(s) = session.take() {
        s.server.abort();
        let mut slot = s.slot.lock().unwrap();
        if let Some(running) = slot.running.take() {
            running.store(false, Ordering::Relaxed);
        }
        // Dropping the VirtualDisplay releases the CGVirtualDisplay and the
        // monitor disappears from System Settings. No-op if a tablet never
        // connected (display was never created).
        slot.display = None;
        slot.frames = None;
    }
}

fn generate_token() -> String {
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..24)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}

/// Short enough to type on a TV remote / projector keyboard — for devices
/// that can't scan the QR code. Brute-forcing is blunted server-side by a
/// lockout after repeated wrong guesses, and the PIN dies after first use.
fn generate_pin() -> String {
    let mut rng = rand::thread_rng();
    format!("{:06}", rng.gen_range(0..1_000_000u32))
}

/// Best local address to show the tablet. WiFi uses the default-route IP;
/// cable mode prefers a USB/bridge interface when one is up.
fn pick_host(mode: &str) -> String {
    if mode == "cable" {
        if let Ok(ifaces) = local_ip_address::list_afinet_netifas() {
            for (name, ip) in &ifaces {
                if ip.is_ipv4() && (name.starts_with("bridge") || name.starts_with("en") && name != "en0")
                {
                    if !ip.is_loopback() && !ip.to_string().starts_with("169.254") {
                        return ip.to_string();
                    }
                }
            }
        }
    }
    local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "localhost".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Creates a real virtual display, waits for WindowServer to bring it up,
    // captures a frame from it, and JPEG-encodes it — the full pipeline minus
    // the network. The display appears on screen for ~2 seconds.
    #[test]
    #[ignore = "requires real display and Screen Recording permission"]
    fn virtual_display_end_to_end() {
        let vd = virtual_display::VirtualDisplay::create("Screen Bridge Test", 1366, 1024)
            .expect("CGVirtualDisplay creation failed");
        assert_ne!(vd.display_id, 0, "got a null display id");
        std::thread::sleep(std::time::Duration::from_millis(2000));

        let active = core_graphics::display::CGDisplay::active_displays()
            .expect("could not list displays");
        assert!(
            active.contains(&vd.display_id),
            "virtual display {} not in active list {:?}",
            vd.display_id,
            active
        );

        let img = core_graphics::display::CGDisplay::new(vd.display_id)
            .image()
            .expect("could not capture virtual display");
        let (w, h) = (img.width() as u32, img.height() as u32);
        assert!(w >= 1366 && h >= 1024, "unexpected capture size {w}x{h}");

        let bpr = img.bytes_per_row() as usize;
        let data = img.data();
        let bytes = data.bytes();
        let (wu, hu) = (w as usize, h as usize);
        let mut rgb = vec![0u8; wu * hu * 3];
        for row in 0..hu {
            for x in 0..wu {
                let s = row * bpr + x * 4;
                let d = (row * wu + x) * 3;
                rgb[d] = bytes[s + 2];
                rgb[d + 1] = bytes[s + 1];
                rgb[d + 2] = bytes[s];
            }
        }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 58)
            .encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
            .expect("jpeg encode failed");
        assert!(jpeg.len() > 1000, "suspiciously small jpeg: {}", jpeg.len());
        println!("OK: display id {}, capture {w}x{h}, jpeg {} bytes", vd.display_id, jpeg.len());
    }

    // Creates a real system-audio process tap + private aggregate device,
    // starts IO, and verifies actual PCM chunks arrive on the broadcast
    // channel within a few seconds — proves the full CoreAudio pipeline
    // (tap -> aggregate device -> IOProc) end to end, not just that it
    // compiles. Chunks arrive continuously regardless of whether audio is
    // audibly playing (silence is still a stream of zeroed samples), so this
    // doesn't require anything to be playing during the test run.
    #[test]
    #[ignore = "requires real system-audio capture permission"]
    fn audio_capture_end_to_end() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<std::sync::Arc<Vec<u8>>>(32);
        // Unmuted so a test run never leaves the Mac silent if it panics
        // between start and drop.
        let (capture, sample_rate, channels) =
            audio_out::start(tx, false).expect("audio capture failed to start");
        assert!(sample_rate > 0.0, "invalid sample rate: {sample_rate}");
        assert!(
            channels == 1 || channels == 2,
            "unexpected channel count: {channels}"
        );
        println!("tap running at {sample_rate} Hz, {channels} ch");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();

        let mut chunks_received = 0;
        let mut total_bytes = 0usize;
        rt.block_on(async {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await
                {
                    Ok(Ok(chunk)) => {
                        chunks_received += 1;
                        total_bytes += chunk.len();
                        assert_eq!(chunk.len() % 4, 0, "Float32 chunk not 4-byte aligned");
                    }
                    _ => continue,
                }
            }
        });

        drop(capture);
        assert!(
            chunks_received > 0,
            "received zero audio chunks in 3 seconds — tap/aggregate/IOProc pipeline isn't delivering"
        );
        println!("OK: {chunks_received} chunks, {total_bytes} total bytes");
    }

    // Diagnostic for the "tablet-only" audio routing mode (CATapMuted): the
    // e2e test above only ever exercises mute_local=false. This plays a real
    // system sound and measures peak sample amplitude in both modes to
    // confirm CATapMuted only silences the Mac's hardware output — not the
    // data the tap itself delivers — as documented ("describes the playback
    // behavior of the process being tapped"). If muted mode measured zero
    // amplitude here, tablet-only routing would be silently broken.
    // #[ignore] — plays an audible system sound, not for routine CI runs.
    #[test]
    #[ignore]
    fn audio_capture_muted_delivers_real_samples() {
        fn capture_peak(muted: bool) -> f32 {
            let (tx, mut rx) = tokio::sync::broadcast::channel::<std::sync::Arc<Vec<u8>>>(32);
            let (capture, _rate, _ch) =
                audio_out::start(tx, muted).expect("audio capture failed to start");

            // Let the tap/aggregate device finish warming up before anything
            // plays — a short system click risked landing entirely inside
            // that startup window with no overlap to capture.
            std::thread::sleep(std::time::Duration::from_millis(300));

            // Several seconds of real speech instead of a ~1s click, so
            // there's no race between playback and the sampling window.
            let mut player = std::process::Command::new("say")
                .arg("one two three four five six seven eight nine ten")
                .spawn()
                .expect("say failed to launch");

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap();

            let mut peak = 0f32;
            rt.block_on(async {
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(5000);
                while tokio::time::Instant::now() < deadline {
                    if let Ok(Ok(chunk)) =
                        tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await
                    {
                        for f in chunk.chunks_exact(4) {
                            let v = f32::from_le_bytes([f[0], f[1], f[2], f[3]]).abs();
                            if v > peak {
                                peak = v;
                            }
                        }
                    }
                }
            });

            let _ = player.wait();
            drop(capture);
            peak
        }

        let peak_unmuted = capture_peak(false);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let peak_muted = capture_peak(true);

        println!("peak_unmuted={peak_unmuted:.6}  peak_muted={peak_muted:.6}");
        assert!(
            peak_unmuted > 0.001,
            "control failed: unmuted tap captured no real signal ({peak_unmuted}) — afplay/tap timing issue, not a muted-mode finding"
        );
        assert!(
            peak_muted > 0.001,
            "CATapMuted is silencing the CAPTURED data, not just Mac hardware output (peak={peak_muted}) — tablet-only routing needs a different mute mechanism"
        );
    }
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            discovery: Discovery::start(),
            session: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            list_devices,
            get_status,
            connect_display,
            disconnect_display
        ])
        .run(tauri::generate_context!())
        .expect("error while running Screen Bridge");
}
