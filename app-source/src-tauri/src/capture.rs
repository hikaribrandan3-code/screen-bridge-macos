//! Captures the virtual display and encodes JPEG frames for streaming.

use core_graphics::display::CGDisplay;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub struct Frame {
    /// `None` when the screen content is identical to the previous tick —
    /// clients keep showing their last-drawn frame and just get the (cheap)
    /// cursor update below.
    pub jpeg: Option<Vec<u8>>,
    pub cursor_x: i32,
    pub cursor_y: i32,
    pub cursor_visible: bool,
    pub width: u32,
    pub height: u32,
}

impl Frame {
    pub fn to_json(&self) -> Vec<u8> {
        format!(
            r#"{{"x":{},"y":{},"visible":{},"w":{},"h":{}}}"#,
            self.cursor_x, self.cursor_y, self.cursor_visible, self.width, self.height
        )
        .into_bytes()
    }
}

/// Returns the cursor's current position local to `display_id` (top-left
/// origin, matching the captured image's pixel space), and whether the
/// cursor is actually over that display right now. CGEvent's `location()`
/// is in *global* coordinate space across all displays, so we subtract the
/// target display's own origin to make it local.
fn cursor_position_for_display(display_id: u32) -> (i32, i32, bool) {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let global = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .and_then(CGEvent::new)
        .map(|e| e.location());

    let Ok(global) = global else {
        return (0, 0, false);
    };

    let bounds = CGDisplay::new(display_id).bounds();
    let local_x = global.x - bounds.origin.x;
    let local_y = global.y - bounds.origin.y;
    let visible = local_x >= 0.0
        && local_y >= 0.0
        && local_x < bounds.size.width
        && local_y < bounds.size.height;

    (local_x as i32, local_y as i32, visible)
}

/// Cheap FNV-1a hash used purely to detect "did the screen change" without
/// cloning the entire raw capture (up to ~16MB+ at Retina sizes) every tick
/// just to remember it for next tick's comparison — same correctness
/// guarantee as a full byte compare, none of the allocation/copy cost.
fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn start(
    display_id: u32,
    tx: broadcast::Sender<Frame>,
    running: Arc<AtomicBool>,
    fps: u32,
    quality: u8,
) {
    std::thread::spawn(move || {
        let display = CGDisplay::new(display_id);

        // The user's chosen quality/fps are treated as a ceiling, not a fixed
        // value: if capture+encode can't keep up, step down from here rather
        // than silently falling further behind every tick, and recover back
        // up toward the ceiling once the Mac keeps up again. Never exceeds
        // what was actually selected.
        let fps_ceiling = fps.max(1);
        let quality_ceiling = quality;
        let fps_floor = (fps_ceiling / 3).max(10).min(fps_ceiling);
        let quality_floor = quality_ceiling.saturating_sub(20).max(25).min(quality_ceiling);

        let mut current_fps = fps_ceiling;
        let mut current_quality = quality_ceiling;
        let mut interval = Duration::from_millis(1000 / current_fps as u64);
        let mut overrun_streak = 0u32;
        let mut healthy_streak = 0u32;

        let mut last_hash: Option<u64> = None;
        let mut last_cursor: Option<(i32, i32, bool)> = None;

        while running.load(Ordering::Relaxed) {
            let started = Instant::now();

            // Skip the expensive capture entirely while nobody is watching.
            if tx.receiver_count() == 0 {
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }

            if let Some(img) = display.image() {
                let w = img.width() as usize;
                let h = img.height() as usize;
                let bpr = img.bytes_per_row() as usize;
                let data = img.data();
                let bytes = data.bytes();

                let cursor = cursor_position_for_display(display_id);
                let (cursor_x, cursor_y, cursor_visible) = cursor;

                // Most desktop-use ticks change nothing on screen — compare
                // the raw capture against the last one before paying for
                // BGRA->RGB conversion + JPEG encoding at all.
                let hash = fnv1a_hash(bytes);
                let content_changed = last_hash != Some(hash);
                let cursor_changed = last_cursor != Some(cursor);

                let jpeg = if content_changed {
                    // CGDisplay images are BGRA (32-bit little endian); JPEG wants RGB.
                    let mut rgb = vec![0u8; w * h * 3];
                    for row in 0..h {
                        let src_off = row * bpr;
                        let dst_off = row * w * 3;
                        for x in 0..w {
                            let s = src_off + x * 4;
                            let d = dst_off + x * 3;
                            rgb[d] = bytes[s + 2];
                            rgb[d + 1] = bytes[s + 1];
                            rgb[d + 2] = bytes[s];
                        }
                    }

                    let mut buf = Vec::new();
                    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
                        &mut buf,
                        current_quality,
                    );
                    let encoded = enc
                        .encode(&rgb, w as u32, h as u32, image::ExtendedColorType::Rgb8)
                        .is_ok();

                    last_hash = Some(hash);
                    encoded.then_some(buf)
                } else {
                    None
                };

                if content_changed || cursor_changed {
                    last_cursor = Some(cursor);
                    let frame = Frame {
                        jpeg,
                        cursor_x,
                        cursor_y,
                        cursor_visible,
                        width: w as u32,
                        height: h as u32,
                    };
                    let _ = tx.send(frame);
                }
            }

            let elapsed = started.elapsed();

            // Adaptive backoff/recovery: sacrifice sharpness before framerate
            // when struggling (smoother motion matters more than crispness),
            // but recover framerate before sharpness once the Mac keeps up.
            if elapsed > interval {
                overrun_streak += 1;
                healthy_streak = 0;
                if overrun_streak >= 8 {
                    overrun_streak = 0;
                    if current_quality > quality_floor {
                        current_quality = current_quality.saturating_sub(10).max(quality_floor);
                    } else if current_fps > fps_floor {
                        current_fps = current_fps.saturating_sub(5).max(fps_floor);
                        interval = Duration::from_millis(1000 / current_fps as u64);
                    }
                }
            } else {
                healthy_streak += 1;
                overrun_streak = 0;
                if healthy_streak >= 60 {
                    healthy_streak = 0;
                    if current_fps < fps_ceiling {
                        current_fps = (current_fps + 5).min(fps_ceiling);
                        interval = Duration::from_millis(1000 / current_fps as u64);
                    } else if current_quality < quality_ceiling {
                        current_quality = (current_quality + 10).min(quality_ceiling);
                    }
                }
            }

            if elapsed < interval {
                std::thread::sleep(interval - elapsed);
            }
        }
    });
}
