//! System audio capture via CoreAudio's process-tap API
//! (`AudioHardwareCreateProcessTap`, macOS 14.2+ — this Mac runs 15.3).
//! Pure Rust + the ObjC runtime, same pattern `virtual_display.rs` already
//! uses for CGVirtualDisplay — no Swift toolchain involved at all, so this
//! sidesteps the broken system Swift Package Manager entirely.
//!
//! Pipeline: CATapDescription (ObjC, msg_send!) -> AudioHardwareCreateProcessTap
//! -> wrap the tap in a private AudioAggregateDevice (taps aren't directly
//! readable; they only become readable once mixed into an aggregate device)
//! -> AudioDeviceCreateIOProcID delivers Float32 PCM buffers on a realtime
//! audio thread -> forwarded raw (no encode) to the broadcast channel.
//!
//! All struct layouts and function signatures below were verified against
//! the actual system headers on this Mac (CoreAudioBaseTypes.h,
//! AudioHardware.h, AudioHardwareTapping.h, CATapDescription.h) rather than
//! recalled from memory — CoreAudio's aggregate-device dictionary API is a
//! notoriously easy place to get subtly wrong.

use core_foundation::array::CFArray;
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use std::ffi::c_void;
use std::os::raw::c_char;
use std::sync::Arc;
use tokio::sync::broadcast;

#[allow(non_camel_case_types)]
type id = *mut Object;

type OSStatus = i32;
type AudioObjectID = u32;
type AudioObjectPropertySelector = u32;
type AudioObjectPropertyScope = u32;
type AudioObjectPropertyElement = u32;
type AudioFormatID = u32;
type AudioFormatFlags = u32;
type AudioDeviceIOProcID = *mut c_void;

const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;
const K_AUDIO_TAP_PROPERTY_FORMAT: u32 = u32::from_be_bytes(*b"tfmt");
const K_AUDIO_FORMAT_LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
const K_AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1 << 0;
// Verified against CoreAudioBaseTypes.h on this Mac: (1U << 5).
const K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED: u32 = 1 << 5;
// Verified against CATapDescription.h on this Mac: CATapMuted = 1 silences
// the Mac's own speakers while the tap keeps receiving the audio stream.
const CA_TAP_UNMUTED: i64 = 0;
const CA_TAP_MUTED: i64 = 1;

#[repr(C)]
struct AudioObjectPropertyAddress {
    m_selector: AudioObjectPropertySelector,
    m_scope: AudioObjectPropertyScope,
    m_element: AudioObjectPropertyElement,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct AudioStreamBasicDescription {
    m_sample_rate: f64,
    m_format_id: AudioFormatID,
    m_format_flags: AudioFormatFlags,
    m_bytes_per_packet: u32,
    m_frames_per_packet: u32,
    m_bytes_per_frame: u32,
    m_channels_per_frame: u32,
    m_bits_per_channel: u32,
    m_reserved: u32,
}

#[repr(C)]
struct SmpteTime {
    m_subframes: i16,
    m_subframe_divisor: i16,
    m_counter: u32,
    m_type: u32,
    m_flags: u32,
    m_hours: i16,
    m_minutes: i16,
    m_seconds: i16,
    m_frames: i16,
}

#[repr(C)]
struct AudioTimeStamp {
    m_sample_time: f64,
    m_host_time: u64,
    m_rate_scalar: f64,
    m_word_clock_time: u64,
    m_smpte_time: SmpteTime,
    m_flags: u32,
    m_reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    m_number_channels: u32,
    m_data_byte_size: u32,
    m_data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    m_number_buffers: u32,
    m_buffers: [AudioBuffer; 1], // variable-length; only index 0 is valid here
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioHardwareCreateProcessTap(description: id, out_tap_id: *mut AudioObjectID) -> OSStatus;
    fn AudioHardwareDestroyProcessTap(tap_id: AudioObjectID) -> OSStatus;
    fn AudioHardwareCreateAggregateDevice(
        description: core_foundation::dictionary::CFDictionaryRef,
        out_device_id: *mut AudioObjectID,
    ) -> OSStatus;
    fn AudioHardwareDestroyAggregateDevice(device_id: AudioObjectID) -> OSStatus;
    fn AudioObjectGetPropertyData(
        object_id: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_data_size: u32,
        qualifier_data: *const c_void,
        io_data_size: *mut u32,
        out_data: *mut c_void,
    ) -> OSStatus;
    fn AudioDeviceCreateIOProcID(
        device_id: AudioObjectID,
        proc_: extern "C" fn(
            AudioObjectID,
            *const AudioTimeStamp,
            *const AudioBufferList,
            *const AudioTimeStamp,
            *mut AudioBufferList,
            *const AudioTimeStamp,
            *mut c_void,
        ) -> OSStatus,
        client_data: *mut c_void,
        out_ioproc_id: *mut AudioDeviceIOProcID,
    ) -> OSStatus;
    fn AudioDeviceStart(device_id: AudioObjectID, ioproc_id: AudioDeviceIOProcID) -> OSStatus;
    fn AudioDeviceStop(device_id: AudioObjectID, ioproc_id: AudioDeviceIOProcID) -> OSStatus;
    fn AudioDeviceDestroyIOProcID(device_id: AudioObjectID, ioproc_id: AudioDeviceIOProcID)
        -> OSStatus;
}

fn nsstring(s: &str) -> id {
    let c = std::ffi::CString::new(s).unwrap();
    unsafe {
        let cls = Class::get("NSString").unwrap();
        msg_send![cls, stringWithUTF8String: c.as_ptr() as *const c_char]
    }
}

fn nsstring_to_string(ns: id) -> String {
    unsafe {
        let ptr: *const c_char = msg_send![ns, UTF8String];
        if ptr.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

/// Everything the realtime IOProc callback needs. Boxed and kept alive for
/// the capture's lifetime; the raw pointer handed to CoreAudio points here.
struct IoCtx {
    tx: broadcast::Sender<Arc<Vec<u8>>>,
    /// Stereo taps can deliver either interleaved (one buffer, LRLR…) or
    /// non-interleaved (two mono buffers) — read from the tap's ASBD rather
    /// than assumed. The wire format is always interleaved Float32.
    non_interleaved: bool,
    channels: u32,
}

pub struct AudioCapture {
    tap_id: AudioObjectID,
    device_id: AudioObjectID,
    ioproc_id: AudioDeviceIOProcID,
    // Kept alive so the client-data pointer handed to CoreAudio stays valid;
    // the IOProc callback dereferences this on every audio cycle.
    _ctx: Box<IoCtx>,
}

unsafe impl Send for AudioCapture {}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.device_id, self.ioproc_id);
            AudioDeviceDestroyIOProcID(self.device_id, self.ioproc_id);
            AudioHardwareDestroyAggregateDevice(self.device_id);
            AudioHardwareDestroyProcessTap(self.tap_id);
        }
    }
}

extern "C" fn io_proc(
    _device: AudioObjectID,
    _now: *const AudioTimeStamp,
    input_data: *const AudioBufferList,
    _input_time: *const AudioTimeStamp,
    _output_data: *mut AudioBufferList,
    _output_time: *const AudioTimeStamp,
    client_data: *mut c_void,
) -> OSStatus {
    if input_data.is_null() || client_data.is_null() {
        return 0;
    }
    unsafe {
        let list = &*input_data;
        if list.m_number_buffers == 0 {
            return 0;
        }
        let ctx = &*(client_data as *const IoCtx);
        // AudioBufferList's buffers array is variable-length; the struct only
        // declares index 0, so further buffers are reached by pointer offset.
        let bufs = list.m_buffers.as_ptr();

        if ctx.non_interleaved && ctx.channels == 2 && list.m_number_buffers >= 2 {
            // Two mono Float32 buffers -> interleave into LRLR for the wire.
            let l = &*bufs;
            let r = &*bufs.add(1);
            if l.m_data.is_null() || r.m_data.is_null() || l.m_data_byte_size == 0 {
                return 0;
            }
            let n = (l.m_data_byte_size.min(r.m_data_byte_size) as usize) / 4;
            let ls = std::slice::from_raw_parts(l.m_data as *const f32, n);
            let rs = std::slice::from_raw_parts(r.m_data as *const f32, n);
            let mut out = Vec::with_capacity(n * 2 * 4);
            for i in 0..n {
                out.extend_from_slice(&ls[i].to_le_bytes());
                out.extend_from_slice(&rs[i].to_le_bytes());
            }
            let _ = ctx.tx.send(Arc::new(out));
        } else {
            // Interleaved (or mono): buffer 0 is already wire-format.
            let buf = &*bufs;
            if buf.m_data.is_null() || buf.m_data_byte_size == 0 {
                return 0;
            }
            let bytes =
                std::slice::from_raw_parts(buf.m_data as *const u8, buf.m_data_byte_size as usize);
            let _ = ctx.tx.send(Arc::new(bytes.to_vec()));
        }
    }
    0
}

/// Starts capturing system audio (stereo, Float32 PCM, whatever sample rate
/// CoreAudio's internal mix engine is running at — read back from the tap
/// itself rather than assumed, since taps deliver at the mix engine's native
/// rate, not a rate we get to pick).
///
/// `mute_local`: true silences the Mac's own speakers while streaming
/// (CATapMuted — "audio follows the display" like a real external monitor);
/// false keeps the Mac playing too.
///
/// Returns the capture handle (keep it alive for the session), the actual
/// sample rate, and the channel count for the client's hello message.
pub fn start(
    tx: broadcast::Sender<Arc<Vec<u8>>>,
    mute_local: bool,
) -> Result<(AudioCapture, f64, u32), String> {
    unsafe {
        let desc_cls =
            Class::get("CATapDescription").ok_or("CATapDescription unavailable (needs macOS 14.2+)")?;
        let desc: id = msg_send![desc_cls, alloc];
        let empty_array: id = {
            let cls = Class::get("NSArray").unwrap();
            msg_send![cls, array]
        };
        // Stereo (verified against CATapDescription.h on this Mac) — a TV or
        // tablet deserves both channels, and the mix engine is stereo anyway.
        let desc: id = msg_send![desc, initStereoGlobalTapButExcludeProcesses: empty_array];
        if desc.is_null() {
            return Err("could not create CATapDescription".into());
        }
        let _: () = msg_send![desc, setPrivate: true];
        let mute = if mute_local { CA_TAP_MUTED } else { CA_TAP_UNMUTED };
        let _: () = msg_send![desc, setMuteBehavior: mute];

        let mut tap_id: AudioObjectID = 0;
        let status = AudioHardwareCreateProcessTap(desc, &mut tap_id);
        if status != 0 {
            return Err(format!("AudioHardwareCreateProcessTap failed: {status}"));
        }

        // Read back the tap's actual delivery format instead of assuming one.
        let mut asbd = AudioStreamBasicDescription::default();
        let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
        let addr = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_TAP_PROPERTY_FORMAT,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        let status = AudioObjectGetPropertyData(
            tap_id,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut asbd as *mut _ as *mut c_void,
        );
        if status != 0 {
            AudioHardwareDestroyProcessTap(tap_id);
            return Err(format!("could not read tap format: {status}"));
        }
        if asbd.m_format_id != K_AUDIO_FORMAT_LINEAR_PCM
            || asbd.m_format_flags & K_AUDIO_FORMAT_FLAG_IS_FLOAT == 0
        {
            AudioHardwareDestroyProcessTap(tap_id);
            return Err("tap delivered an unexpected (non-Float32-PCM) format".into());
        }

        let tap_uuid: id = msg_send![desc, UUID];
        let tap_uuid_str: id = msg_send![tap_uuid, UUIDString];
        let tap_uid = nsstring_to_string(tap_uuid_str);

        let sub_tap_dict = CFDictionary::from_CFType_pairs(&[(
            CFString::new("uid"),
            CFString::new(&tap_uid).as_CFType(),
        )]);
        let taps_array = CFArray::from_CFTypes(&[sub_tap_dict.as_CFType()]);

        let agg_dict = CFDictionary::from_CFType_pairs(&[
            (
                CFString::new("name"),
                CFString::new("Screen Bridge Audio Tap").as_CFType(),
            ),
            (
                CFString::new("uid"),
                CFString::new("com.screenbridge.audiotap").as_CFType(),
            ),
            (
                CFString::new("private"),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                CFString::new("tapautostart"),
                CFBoolean::true_value().as_CFType(),
            ),
            (CFString::new("taps"), taps_array.as_CFType()),
        ]);

        let mut device_id: AudioObjectID = 0;
        let status = AudioHardwareCreateAggregateDevice(
            agg_dict.as_concrete_TypeRef(),
            &mut device_id,
        );
        if status != 0 {
            AudioHardwareDestroyProcessTap(tap_id);
            return Err(format!("AudioHardwareCreateAggregateDevice failed: {status}"));
        }

        let ctx = Box::new(IoCtx {
            tx,
            non_interleaved: asbd.m_format_flags & K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED != 0,
            channels: asbd.m_channels_per_frame.max(1),
        });
        let client_data = ctx.as_ref() as *const IoCtx as *mut c_void;

        let mut ioproc_id: AudioDeviceIOProcID = std::ptr::null_mut();
        let status = AudioDeviceCreateIOProcID(device_id, io_proc, client_data, &mut ioproc_id);
        if status != 0 {
            AudioHardwareDestroyAggregateDevice(device_id);
            AudioHardwareDestroyProcessTap(tap_id);
            return Err(format!("AudioDeviceCreateIOProcID failed: {status}"));
        }

        let status = AudioDeviceStart(device_id, ioproc_id);
        if status != 0 {
            AudioDeviceDestroyIOProcID(device_id, ioproc_id);
            AudioHardwareDestroyAggregateDevice(device_id);
            AudioHardwareDestroyProcessTap(tap_id);
            return Err(format!("AudioDeviceStart failed: {status}"));
        }

        let channels = ctx.channels;
        Ok((
            AudioCapture {
                tap_id,
                device_id,
                ioproc_id,
                _ctx: ctx,
            },
            asbd.m_sample_rate,
            channels,
        ))
    }
}
