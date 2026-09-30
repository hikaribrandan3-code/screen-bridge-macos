//! True macOS extended display via the private CGVirtualDisplay API
//! (the same mechanism BetterDisplay / BetterDummy use). The display shows up
//! in System Settings → Displays as a real monitor, so window dragging and
//! cursor traversal are handled natively by macOS.
//!
//! All classes are resolved at runtime with `Class::get`, so nothing links
//! against private symbols — if Apple removes the API we fail with a clean
//! error instead of a crash at launch.

use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use std::ffi::{c_void, CString};

#[allow(non_camel_case_types)]
type id = *mut Object;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

unsafe impl objc::Encode for CGPoint {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{CGPoint=dd}") }
    }
}

unsafe impl objc::Encode for CGSize {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{CGSize=dd}") }
    }
}

extern "C" {
    static _dispatch_main_q: c_void;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

fn main_queue() -> id {
    unsafe { &_dispatch_main_q as *const _ as id }
}

fn nsstring(s: &str) -> id {
    let c = CString::new(s).unwrap();
    unsafe {
        let cls = Class::get("NSString").unwrap();
        msg_send![cls, stringWithUTF8String: c.as_ptr()]
    }
}

/// Prompts for Screen Recording permission if not yet granted.
/// Returns true when access is available.
pub fn ensure_capture_access() -> bool {
    unsafe {
        if CGPreflightScreenCaptureAccess() {
            true
        } else {
            CGRequestScreenCaptureAccess()
        }
    }
}

pub struct VirtualDisplay {
    display: id,
    settings: id,
    pub display_id: u32,
}

// The objc objects are only touched from create/drop; the raw pointers just
// need to live inside the app state across threads.
unsafe impl Send for VirtualDisplay {}

impl VirtualDisplay {
    pub fn create(name: &str, width: u32, height: u32) -> Result<Self, String> {
        unsafe {
            let desc_cls = Class::get("CGVirtualDisplayDescriptor")
                .ok_or("virtual display API unavailable on this macOS version")?;
            let display_cls = Class::get("CGVirtualDisplay")
                .ok_or("virtual display API unavailable on this macOS version")?;
            let settings_cls = Class::get("CGVirtualDisplaySettings")
                .ok_or("virtual display API unavailable on this macOS version")?;
            let mode_cls = Class::get("CGVirtualDisplayMode")
                .ok_or("virtual display API unavailable on this macOS version")?;

            let desc: id = msg_send![desc_cls, alloc];
            let desc: id = msg_send![desc, init];
            let _: () = msg_send![desc, setName: nsstring(name)];
            let _: () = msg_send![desc, setQueue: main_queue()];
            let _: () = msg_send![desc, setMaxPixelsWide: width];
            let _: () = msg_send![desc, setMaxPixelsHigh: height];
            // ~110 ppi physical size so macOS picks a sane default scale
            let mm_per_px = 25.4 / 110.0;
            let size = CGSize {
                width: width as f64 * mm_per_px,
                height: height as f64 * mm_per_px,
            };
            let _: () = msg_send![desc, setSizeInMillimeters: size];
            let _: () = msg_send![desc, setSerialNum: 0x0001u32];
            let _: () = msg_send![desc, setProductID: 0x0001u32];
            let _: () = msg_send![desc, setVendorID: 0xF0F0u32];
            // sRGB-ish primaries (same values BetterDummy ships)
            let _: () = msg_send![desc, setRedPrimary: CGPoint { x: 0.6797, y: 0.3203 }];
            let _: () = msg_send![desc, setGreenPrimary: CGPoint { x: 0.2148, y: 0.7109 }];
            let _: () = msg_send![desc, setBluePrimary: CGPoint { x: 0.1406, y: 0.0703 }];
            let _: () = msg_send![desc, setWhitePoint: CGPoint { x: 0.3128, y: 0.3292 }];

            let display: id = msg_send![display_cls, alloc];
            let display: id = msg_send![display, initWithDescriptor: desc];
            let _: () = msg_send![desc, release];
            if display.is_null() {
                return Err("could not create virtual display".into());
            }

            let settings: id = msg_send![settings_cls, alloc];
            let settings: id = msg_send![settings, init];
            let _: () = msg_send![settings, setHiDPI: 0u32];

            let mode: id = msg_send![mode_cls, alloc];
            let mode: id =
                msg_send![mode, initWithWidth: width as usize height: height as usize refreshRate: 60.0f64];
            let array_cls = Class::get("NSArray").unwrap();
            let modes: id = msg_send![array_cls, arrayWithObject: mode];
            let _: () = msg_send![settings, setModes: modes];
            let _: () = msg_send![mode, release];

            let applied: bool = msg_send![display, applySettings: settings];
            if !applied {
                let _: () = msg_send![settings, release];
                let _: () = msg_send![display, release];
                return Err("could not create virtual display (applySettings failed)".into());
            }

            let display_id: u32 = msg_send![display, displayID];
            Ok(VirtualDisplay {
                display,
                settings,
                display_id,
            })
        }
    }
}

impl Drop for VirtualDisplay {
    fn drop(&mut self) {
        unsafe {
            // Releasing the CGVirtualDisplay tears the monitor down.
            let _: () = msg_send![self.settings, release];
            let _: () = msg_send![self.display, release];
        }
    }
}
