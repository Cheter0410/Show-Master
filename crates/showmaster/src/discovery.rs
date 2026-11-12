//! mDNS/Bonjour glue: a background thread that keeps the advertisements and
//! the RTP-MIDI browse list in line with the show settings (read from the
//! published state document, so changes apply without a restart) and hands
//! the status to the runtime for the state document.
//!
//! The thread never touches the engine or audio threads; it only reads the
//! published state (a short read lock) and talks to the `mdns-sd` daemon over
//! non-blocking channels.

use serde_json::{json, Value};
use sm_mdns::{Desired, Mdns, RtpAdvert, Status};
use sm_server::runtime::{Handle, NetStatusFn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

const INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Snapshot {
    /// `remote.mdns` setting.
    enabled: bool,
    status: Status,
}

/// Status shared between the mDNS thread and the runtime.
#[derive(Clone, Default)]
pub struct Shared(Arc<Mutex<Snapshot>>);

impl Shared {
    /// The hook the runtime calls while building the state document.
    pub fn status_fn(&self) -> NetStatusFn {
        let shared = self.0.clone();
        Arc::new(move || {
            let snap = shared.lock().unwrap_or_else(PoisonError::into_inner);
            status_json(&snap)
        })
    }
}

fn status_json(snap: &Snapshot) -> Value {
    let s = &snap.status;
    json!({
        "mdns": {
            "enabled": snap.enabled,
            "active": s.active,
            "host": s.host,
            "url": s.url,
            "rtp_advertised": s.rtp_advertised,
            "rtp_instance": s.rtp_instance,
            "error": s.error,
        },
        "discovered": s.discovered,
    })
}

/// What to advertise, from the published state document.
/// `http_port` is the port the web server really listens on, `None` if it
/// only listens on this machine.
pub fn desired(state: &Value, http_port: Option<u16>) -> (bool, Desired) {
    let web_on = state["show"]["settings"]["remote"]["mdns"]
        .as_bool()
        .unwrap_or(true);
    let rtp = &state["midi"]["rtp"];
    let name = rtp["name"].as_str().unwrap_or_default();
    let running = rtp["enabled"].as_bool().unwrap_or(false)
        && state["midi"]["ports"].as_array().is_some_and(|ports| {
            ports
                .iter()
                .any(|p| p.as_str() == Some(&format!("rtp:{name}")))
        });
    let advertise = running
        && state["show"]["rtp_midi"]["advertise"]
            .as_bool()
            .unwrap_or(true);
    let port = rtp["port"]
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0);
    (
        web_on,
        Desired {
            http_port: http_port.filter(|_| web_on),
            rtp: port.filter(|_| advertise).map(|port| RtpAdvert {
                name: name.to_owned(),
                port,
            }),
            browse_rtp: advertise,
        },
    )
}

/// The running mDNS thread; stopping it withdraws the advertisements.
pub struct Advertiser {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Advertiser {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Starts the mDNS thread. Failure to start is logged; the app runs without.
pub fn start(handle: Handle, shared: Shared, http_port: Option<u16>) -> Option<Advertiser> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::Builder::new()
        .name("sm-mdns".into())
        .spawn(move || run(&handle, &shared, http_port, &flag));
    match thread {
        Ok(t) => Some(Advertiser {
            stop,
            thread: Some(t),
        }),
        Err(e) => {
            tracing::warn!("cannot start the mDNS thread: {e}");
            None
        }
    }
}

fn run(handle: &Handle, shared: &Shared, http_port: Option<u16>, stop: &AtomicBool) {
    let mut mdns = Mdns::new();
    let mut last: Option<Desired> = None;
    let mut took_over = false;
    while !stop.load(Ordering::SeqCst) {
        // A backup that has just taken over claims the plain name again
        // (`showmaster.local`, it was `showmaster-2.local` while the main ran),
        // so tablets that type the name reach it.
        let taken = handle
            .published
            .read()
            .ok()
            .is_some_and(|p| p.state["remote"]["link"]["taken_over"] == true);
        if taken && !took_over {
            tracing::info!("backup took over: announcing the plain Bonjour name again");
            mdns.shutdown();
            std::thread::sleep(Duration::from_millis(200));
            mdns = Mdns::new();
        }
        took_over = taken;
        let wanted = handle
            .published
            .read()
            .ok()
            .map(|p| desired(&p.state, http_port));
        if let Some((enabled, wanted)) = wanted {
            // `apply` is cheap when nothing changed, but also retries a daemon
            // that failed to start; call it every round.
            mdns.apply(&wanted);
            if last.as_ref() != Some(&wanted) {
                tracing::debug!(?wanted, "mDNS settings");
                last = Some(wanted);
            }
            mdns.poll();
            let mut snap = shared.0.lock().unwrap_or_else(PoisonError::into_inner);
            snap.enabled = enabled;
            snap.status = mdns.status();
        }
        std::thread::sleep(INTERVAL);
    }
    mdns.shutdown();
    // Give the daemon a moment to send the goodbye packets.
    std::thread::sleep(Duration::from_millis(200));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(remote_mdns: Option<bool>, rtp_enabled: bool, advertise: bool) -> Value {
        let mut remote = json!({ "enabled": true });
        if let Some(on) = remote_mdns {
            remote["mdns"] = json!(on);
        }
        json!({
            "show": { "settings": { "remote": remote }, "rtp_midi": { "advertise": advertise } },
            "midi": {
                "ports": if rtp_enabled { json!(["rtp:Rig"]) } else { json!([]) },
                "rtp": { "enabled": rtp_enabled, "name": "Rig", "port": 5004 },
            },
        })
    }

    #[test]
    fn settings_map_to_advertisements() {
        let (on, d) = desired(&state(None, true, true), Some(7400));
        assert!(on, "remote.mdns defaults to on");
        assert_eq!(d.http_port, Some(7400));
        assert_eq!(
            d.rtp,
            Some(RtpAdvert {
                name: "Rig".into(),
                port: 5004
            })
        );
        assert!(d.browse_rtp);

        let (on, d) = desired(&state(Some(false), true, false), Some(7400));
        assert!(!on);
        assert!(d.is_idle());

        // Web server only on this machine (--local-only / remote off).
        let (_, d) = desired(&state(Some(true), false, true), None);
        assert!(d.is_idle());

        // RTP-MIDI enabled but the session did not start (port in use).
        let mut s = state(None, true, true);
        s["midi"]["ports"] = json!([]);
        let (_, d) = desired(&s, None);
        assert_eq!(d.rtp, None);
        assert!(!d.browse_rtp);
    }

    #[test]
    fn status_document() {
        let snap = Snapshot {
            enabled: true,
            status: Status {
                active: true,
                host: "showmaster.local".into(),
                url: Some("http://showmaster.local:7400".into()),
                discovered: vec![sm_mdns::Discovered {
                    name: "Mac".into(),
                    addr: "192.168.1.5:5004".into(),
                    host: "mac.local".into(),
                }],
                ..Status::default()
            },
        };
        let v = status_json(&snap);
        assert_eq!(v["mdns"]["url"], "http://showmaster.local:7400");
        assert_eq!(v["mdns"]["enabled"], true);
        assert_eq!(v["discovered"][0]["addr"], "192.168.1.5:5004");
    }
}
