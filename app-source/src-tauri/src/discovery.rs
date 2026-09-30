//! Bonjour/mDNS browsing to populate the "Select Device" dropdown with
//! tablets and other devices visible on the local network. Also advertises
//! the Screen Bridge service itself so TVs/projectors can find it at a
//! memorable hostname (screenbridge.local) instead of typing an IP.

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Serialize)]
pub struct Device {
    pub name: String,
    pub kind: String, // "apple" | "android" | "generic"
}

#[derive(Clone)]
pub struct Discovery {
    devices: Arc<Mutex<HashMap<String, Device>>>,
}

const SERVICES: &[(&str, &str)] = &[
    ("_companion-link._tcp.local.", "apple"),
    ("_rdlink._tcp.local.", "apple"),
    ("_airplay._tcp.local.", "apple"),
    ("_googlecast._tcp.local.", "android"),
    ("_androidtvremote2._tcp.local.", "android"),
];

impl Discovery {
    pub fn start() -> Self {
        let devices: Arc<Mutex<HashMap<String, Device>>> = Arc::new(Mutex::new(HashMap::new()));

        if let Ok(daemon) = ServiceDaemon::new() {
            for (service, kind) in SERVICES {
                let rx = match daemon.browse(service) {
                    Ok(rx) => rx,
                    Err(_) => continue,
                };
                let devices = devices.clone();
                let kind = kind.to_string();
                std::thread::spawn(move || {
                    while let Ok(event) = rx.recv() {
                        match event {
                            ServiceEvent::ServiceResolved(info) => {
                                let name = instance_name(info.get_fullname());
                                if !name.is_empty() {
                                    devices.lock().unwrap().insert(
                                        info.get_fullname().to_string(),
                                        Device {
                                            name,
                                            kind: kind.clone(),
                                        },
                                    );
                                }
                            }
                            ServiceEvent::ServiceRemoved(_, fullname) => {
                                devices.lock().unwrap().remove(&fullname);
                            }
                            _ => {}
                        }
                    }
                });
            }
            // Keep the daemon alive for the lifetime of the app.
            std::mem::forget(daemon);
        }

        Discovery { devices }
    }

    pub fn list(&self) -> Vec<Device> {
        let map = self.devices.lock().unwrap();
        let mut seen = HashMap::new();
        for d in map.values() {
            seen.entry(d.name.clone()).or_insert_with(|| d.clone());
        }
        let mut list: Vec<Device> = seen.into_values().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }
}

/// Advertise Screen Bridge on mDNS so tablets/TVs can reach it at
/// `screenbridge.local:port` without needing to know the IP address.
/// The service is published for the lifetime of the returned `ServiceDaemon`;
/// once it's dropped, mDNS stops advertising. So the daemon is typically
/// stored in a session or static state so it persists for the whole session.
pub fn advertise_service(port: u16) -> Option<ServiceDaemon> {
    let daemon = ServiceDaemon::new().ok()?;

    let service_name = "screenbridge";
    let service_type = "_http._tcp.local.";
    let fullname = format!("{}._http._tcp.local.", service_name);

    let mut properties = HashMap::new();
    properties.insert("path".to_string(), "/".to_string());

    // Let mdns autodiscover the local IP instead of hardcoding it.
    let info = ServiceInfo::new(
        service_type,
        service_name,
        &fullname,
        "",
        port,
        Some(properties),
    )
    .ok()?
    .enable_addr_auto();

    daemon.register(info).ok()?;
    Some(daemon)
}

/// "iPad de Sarah._companion-link._tcp.local." → "iPad de Sarah"
fn instance_name(fullname: &str) -> String {
    fullname
        .split("._")
        .next()
        .unwrap_or("")
        .replace("\\032", " ")
        .replace("\\ ", " ")
}
