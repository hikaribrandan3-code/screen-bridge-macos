//! Turns messages from the tablet into synthetic macOS mouse/keyboard
//! events, posted at the HID level so they behave exactly like real
//! hardware input — window focus, dragging, and app shortcuts all work.
//!
//! Coordinates in incoming messages are already in Mac virtual-display
//! local pixel space (the same space `capture::cursor_position_for_display`
//! reports in) — the tablet computes this using the inverse of the cursor
//! overlay math, so no DPR/CSS-pixel awareness is needed here.

use core_graphics::display::CGDisplay;
use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, KeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};

/// Real-world usage is one active controller at a time, so a single shared
/// "is the left button currently held" flag is enough to pick between
/// MouseMoved and LeftMouseDragged.
static LEFT_DOWN: AtomicBool = AtomicBool::new(false);

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ClientMsg {
    #[allow(dead_code)]
    Hello { vw: u32, vh: u32 },
    Move { x: f64, y: f64 },
    Down { x: f64, y: f64, button: u8 },
    Up { x: f64, y: f64, button: u8 },
    Scroll { dx: f64, dy: f64 },
    Key { key: String, down: bool },
    Text { s: String },
}

pub fn handle_message(display_id: u32, text: &str) {
    let Ok(msg) = serde_json::from_str::<ClientMsg>(text) else {
        return;
    };
    match msg {
        // Already consumed by server.rs before the display existed; a
        // stray repeat after streaming has started is a no-op.
        ClientMsg::Hello { .. } => {}
        ClientMsg::Move { x, y } => {
            let event_type = if LEFT_DOWN.load(Ordering::Relaxed) {
                CGEventType::LeftMouseDragged
            } else {
                CGEventType::MouseMoved
            };
            post_mouse(display_id, event_type, x, y, CGMouseButton::Left);
        }
        ClientMsg::Down { x, y, button } => {
            let (event_type, cg_button) = mouse_button(button, true);
            if button == 0 {
                LEFT_DOWN.store(true, Ordering::Relaxed);
            }
            post_mouse(display_id, event_type, x, y, cg_button);
        }
        ClientMsg::Up { x, y, button } => {
            let (event_type, cg_button) = mouse_button(button, false);
            if button == 0 {
                LEFT_DOWN.store(false, Ordering::Relaxed);
            }
            post_mouse(display_id, event_type, x, y, cg_button);
        }
        ClientMsg::Scroll { dx, dy } => post_scroll(dx, dy),
        ClientMsg::Key { key, down } => post_key(&key, down),
        ClientMsg::Text { s } => post_text(&s),
    }
}

fn mouse_button(button: u8, down: bool) -> (CGEventType, CGMouseButton) {
    match (button, down) {
        (2, true) => (CGEventType::RightMouseDown, CGMouseButton::Right),
        (2, false) => (CGEventType::RightMouseUp, CGMouseButton::Right),
        (_, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
        (_, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
    }
}

fn source() -> Option<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()
}

fn global_point(display_id: u32, local_x: f64, local_y: f64) -> CGPoint {
    let bounds = CGDisplay::new(display_id).bounds();
    CGPoint {
        x: bounds.origin.x + local_x,
        y: bounds.origin.y + local_y,
    }
}

fn post_mouse(display_id: u32, event_type: CGEventType, x: f64, y: f64, button: CGMouseButton) {
    let Some(src) = source() else { return };
    let point = global_point(display_id, x, y);
    if let Ok(event) = CGEvent::new_mouse_event(src, event_type, point, button) {
        event.post(CGEventTapLocation::HID);
    }
}

fn post_scroll(dx: f64, dy: f64) {
    use core_graphics::event::ScrollEventUnit;
    let Some(src) = source() else { return };
    if let Ok(event) = CGEvent::new_scroll_event(
        src,
        ScrollEventUnit::PIXEL,
        2,
        dy.round() as i32,
        dx.round() as i32,
        0,
    ) {
        event.post(CGEventTapLocation::HID);
    }
}

/// Special (non-printable) keys go through explicit virtual keycodes;
/// everything else is handled by `post_text` via `set_string`, which is
/// layout/locale independent (the app ships ES/EN/PT).
fn post_key(key: &str, down: bool) {
    let code = match key {
        "Enter" => KeyCode::RETURN,
        "Tab" => KeyCode::TAB,
        "Backspace" => KeyCode::DELETE,
        "Delete" => KeyCode::FORWARD_DELETE,
        "Escape" => KeyCode::ESCAPE,
        "ArrowLeft" => KeyCode::LEFT_ARROW,
        "ArrowRight" => KeyCode::RIGHT_ARROW,
        "ArrowUp" => KeyCode::UP_ARROW,
        "ArrowDown" => KeyCode::DOWN_ARROW,
        " " | "Space" => KeyCode::SPACE,
        "Shift" => KeyCode::SHIFT,
        "Control" => KeyCode::CONTROL,
        "Alt" => KeyCode::OPTION,
        "Meta" => KeyCode::COMMAND,
        // Single printable character with no special handling — post it as
        // text instead of guessing a keycode.
        _ if key.chars().count() == 1 => {
            if down {
                post_text(key);
            }
            return;
        }
        _ => return,
    };

    let Some(src) = source() else { return };
    if let Ok(event) = CGEvent::new_keyboard_event(src, code, down) {
        event.post(CGEventTapLocation::HID);
    }
}

fn post_text(s: &str) {
    let Some(src) = source() else { return };
    // Keycode 0 with an explicit string payload — bypasses keyboard-layout
    // mapping entirely, which matters since this app is already ES/EN/PT.
    if let Ok(down) = CGEvent::new_keyboard_event(src, 0, true) {
        down.set_string(s);
        down.post(CGEventTapLocation::HID);
    }
    if let Some(src) = source() {
        if let Ok(up) = CGEvent::new_keyboard_event(src, 0, false) {
            up.set_string(s);
            up.post(CGEventTapLocation::HID);
        }
    }
}
