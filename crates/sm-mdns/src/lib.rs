//! # sm-mdns
//!
//! Optional mDNS / DNS-SD ("Bonjour", "Zeroconf") support for Show Master:
//!
//! * advertises the web interface as `_http._tcp` (instance
//!   "Show Master on <computer>", TXT `path=/remote`) under a friendly host
//!   name (`showmaster.local`, renamed to `showmaster-2.local` etc. if another
//!   machine already uses it), so tablets can open
//!   `http://showmaster.local:7400/remote`;
//! * advertises the RTP-MIDI session as `_apple-midi._udp` on its control port
//!   (macOS Audio MIDI Setup, iOS, rtpMIDI on Windows list it);
//! * browses for other `_apple-midi._udp` sessions.
//!
//! Everything is **local-link multicast only** (224.0.0.251 / ff02::fb, UDP
//! 5353, TTL 255 per RFC 6762); nothing is sent anywhere else. mDNS is a
//! comfort function: if the network has no multicast, port 5353 cannot be
//! opened or the daemon cannot start, this crate logs and carries on - access
//! by IP address / QR code always works.
//!
//! [`Mdns`] never blocks: the `mdns-sd` daemon runs on its own thread and all
//! calls only queue commands or drain channels. Call [`Mdns::apply`] with the
//! wanted state and [`Mdns::poll`] periodically (e.g. twice a second) from a
//! thread of your own; read the result with [`Mdns::status`].

use mdns_sd::{DaemonEvent, Receiver, ServiceDaemon, ServiceEvent, ServiceInfo, TryRecvError};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Standard mDNS port.
pub const MDNS_PORT: u16 = 5353;
/// DNS-SD type of the web interface.
pub const HTTP_SERVICE: &str = "_http._tcp.local.";
/// DNS-SD type of RTP-MIDI (AppleMIDI) sessions.
pub const APPLE_MIDI_SERVICE: &str = "_apple-midi._udp.local.";
/// Default host label: `showmaster.local`.
pub const DEFAULT_HOST: &str = "showmaster";
/// Path tablets should open, published in the TXT record.
pub const REMOTE_PATH: &str = "/remote";
/// Pause before starting the daemon again after it failed to start.
const RETRY: Duration = Duration::from_secs(30);
/// Longest DNS label.
const MAX_LABEL: usize = 63;

/// An RTP-MIDI session to advertise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpAdvert {
    /// Session name (becomes the Bonjour instance name).
    pub name: String,
    /// Control port.
    pub port: u16,
}

/// What should be advertised / browsed right now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Desired {
    /// Port of the web interface to advertise, `None` = do not advertise.
    pub http_port: Option<u16>,
    /// RTP-MIDI session to advertise.
    pub rtp: Option<RtpAdvert>,
    /// Browse for other RTP-MIDI sessions.
    pub browse_rtp: bool,
}

impl Desired {
    /// `true` if nothing at all is wanted (the daemon is then shut down).
    pub fn is_idle(&self) -> bool {
        self.http_port.is_none() && self.rtp.is_none() && !self.browse_rtp
    }
}

/// Another RTP-MIDI session found on the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Discovered {
    /// Bonjour instance name (normally the session name).
    pub name: String,
    /// `ip:port` of its control port, usable with `rtp_connect`.
    pub addr: String,
    /// Its mDNS host name (e.g. `studio-mac.local`).
    pub host: String,
}

/// Snapshot for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct Status {
    /// The mDNS daemon is running (it may still lack multicast; see `error`).
    pub active: bool,
    /// Host name the web interface is advertised under (e.g. `showmaster.local`).
    pub host: String,
    /// `http://<host>:<port>` while the web interface is advertised.
    pub url: Option<String>,
    /// The RTP-MIDI session is advertised.
    pub rtp_advertised: bool,
    /// Instance name the RTP-MIDI session is advertised under (normally the
    /// session name; "Name (2)" if the name was taken on the network).
    pub rtp_instance: Option<String>,
    /// Last problem (daemon start failure, multicast unavailable, ...).
    pub error: Option<String>,
    /// Other RTP-MIDI sessions, sorted by name.
    pub discovered: Vec<Discovered>,
}

/// Makes a DNS host label from free text: lowercase `a-z`, `0-9` and `-`,
/// at most 63 characters, never empty.
pub fn host_label(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.trim_end_matches('-').chars().take(MAX_LABEL).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        DEFAULT_HOST.to_owned()
    } else {
        out
    }
}

/// Cuts `text` to at most 63 bytes on a character boundary (DNS label limit).
pub fn instance_label(text: &str) -> String {
    let text = text.trim();
    let mut end = text.len().min(MAX_LABEL);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = text.get(..end).unwrap_or_default().trim_end();
    if cut.is_empty() {
        "Show Master".to_owned()
    } else {
        cut.to_owned()
    }
}

/// Instance name of the web interface: "Show Master on <computer>".
pub fn http_instance(computer: &str) -> String {
    let computer = computer.trim().trim_end_matches(".local").trim();
    if computer.is_empty() {
        instance_label("Show Master")
    } else {
        instance_label(&format!("Show Master on {computer}"))
    }
}

/// This computer's name as the operating system reports it.
pub fn computer_name() -> String {
    gethostname::gethostname()
        .to_string_lossy()
        .trim()
        .to_owned()
}

/// TXT properties of the web interface service.
pub fn http_txt() -> Vec<(&'static str, &'static str)> {
    vec![("path", REMOTE_PATH), ("app", "showmaster")]
}

/// Builds the advertisement of the web interface.
pub fn http_service(instance: &str, host: &str, port: u16) -> Result<ServiceInfo, mdns_sd::Error> {
    ServiceInfo::new(
        HTTP_SERVICE,
        instance,
        &fqdn(host),
        (),
        port,
        http_txt().as_slice(),
    )
    .map(ServiceInfo::enable_addr_auto)
}

/// Builds the advertisement of an RTP-MIDI session.
pub fn rtp_service(session: &RtpAdvert, host: &str) -> Result<ServiceInfo, mdns_sd::Error> {
    ServiceInfo::new(
        APPLE_MIDI_SERVICE,
        &instance_label(&session.name),
        &fqdn(host),
        (),
        session.port,
        None::<std::collections::HashMap<String, String>>,
    )
    .map(ServiceInfo::enable_addr_auto)
}

fn fqdn(host: &str) -> String {
    format!(
        "{}.local.",
        host.trim_end_matches('.').trim_end_matches(".local")
    )
}

/// `Name._apple-midi._udp.local.` -> `Name` (DNS escapes removed).
pub fn instance_of(fullname: &str, service: &str) -> String {
    let raw = fullname
        .strip_suffix(service)
        .map(|s| s.strip_suffix('.').unwrap_or(s))
        .unwrap_or(fullname);
    let mut out = Vec::new();
    let bytes = raw.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'\\' {
            let digits = bytes
                .get(i + 1..i + 4)
                .filter(|d| d.iter().all(u8::is_ascii_digit));
            if let Some(value) = digits
                .and_then(|d| std::str::from_utf8(d).ok())
                .and_then(|d| d.parse::<u8>().ok())
            {
                out.push(value);
                i += 4;
                continue;
            }
            if let Some(&next) = bytes.get(i + 1) {
                out.push(next);
                i += 2;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Picks the address to connect to: a non-loopback IPv4 address if there is
/// one, else a loopback one (same machine).
pub fn pick_ipv4(addrs: impl IntoIterator<Item = Ipv4Addr>) -> Option<Ipv4Addr> {
    let mut all: Vec<Ipv4Addr> = addrs.into_iter().collect();
    all.sort();
    all.iter()
        .find(|a| !a.is_loopback() && !a.is_link_local())
        .or_else(|| all.iter().find(|a| !a.is_loopback()))
        .or_else(|| all.first())
        .copied()
}

struct Registered {
    /// What was asked for (to detect changes).
    key: (String, u16),
    fullname: String,
}

/// mDNS advertisement and discovery; see the crate documentation.
pub struct Mdns {
    port: u16,
    /// Wanted host label (`showmaster`) and the one in use after conflicts.
    host: String,
    computer: String,
    daemon: Option<ServiceDaemon>,
    monitor: Option<Receiver<DaemonEvent>>,
    retry_at: Option<Instant>,
    error: Option<String>,
    http: Option<Registered>,
    rtp: Option<Registered>,
    browse: Option<Receiver<ServiceEvent>>,
    found: BTreeMap<String, Discovered>,
    url: Option<String>,
}

impl std::fmt::Debug for Mdns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mdns")
            .field("port", &self.port)
            .field("host", &self.host)
            .field("active", &self.daemon.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for Mdns {
    fn default() -> Self {
        Self::new()
    }
}

impl Mdns {
    /// Standard port 5353, host `showmaster.local`, this computer's name.
    pub fn new() -> Self {
        Self::with_options(MDNS_PORT, DEFAULT_HOST, &computer_name())
    }

    /// Custom mDNS port (tests), host label and computer name.
    pub fn with_options(port: u16, host: &str, computer: &str) -> Self {
        Self {
            port,
            host: host_label(host),
            computer: computer.to_owned(),
            daemon: None,
            monitor: None,
            retry_at: None,
            error: None,
            http: None,
            rtp: None,
            browse: None,
            found: BTreeMap::new(),
            url: None,
        }
    }

    /// Brings advertisements and browsing in line with `desired`. Never blocks.
    pub fn apply(&mut self, desired: &Desired) {
        if desired.is_idle() {
            if self.daemon.is_some() {
                self.shutdown();
            }
            self.retry_at = None;
            return;
        }
        let Some(daemon) = self.ensure_daemon() else {
            return;
        };
        let host = self.host.clone();

        let want_http = desired
            .http_port
            .map(|port| (http_instance(&self.computer), port));
        let result = sync_service(&daemon, &mut self.http, want_http, |(name, port)| {
            http_service(name, &host, *port)
        });
        self.note(result, "web interface");
        self.url = self
            .http
            .as_ref()
            .map(|r| format!("http://{host}.local:{}", r.key.1));

        let want_rtp = desired
            .rtp
            .as_ref()
            .map(|r| (instance_label(&r.name), r.port));
        let result = sync_service(&daemon, &mut self.rtp, want_rtp, |(name, port)| {
            rtp_service(
                &RtpAdvert {
                    name: name.clone(),
                    port: *port,
                },
                &host,
            )
        });
        self.note(result, "RTP-MIDI session");

        match (desired.browse_rtp, self.browse.is_some()) {
            (true, false) => match daemon.browse(APPLE_MIDI_SERVICE) {
                Ok(rx) => self.browse = Some(rx),
                Err(e) => self.note(Err(e), "RTP-MIDI browsing"),
            },
            (false, true) => {
                let _ = daemon.stop_browse(APPLE_MIDI_SERVICE);
                self.browse = None;
                self.found.clear();
            }
            _ => {}
        }
    }

    /// Takes in discovery results and daemon events. Never blocks.
    pub fn poll(&mut self) {
        let mut events = Vec::new();
        if let Some(rx) = &self.browse {
            loop {
                match rx.try_recv() {
                    Ok(event) => events.push(event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        // Channel closed (daemon gone or receiver too slow): browse again.
                        self.browse = None;
                        break;
                    }
                }
            }
        }
        for event in events {
            self.on_service_event(event);
        }
        let mut renamed = None;
        if let Some(rx) = &self.monitor {
            while let Ok(event) = rx.try_recv() {
                match event {
                    // Reported once per interface; act (and log) on the first.
                    DaemonEvent::NameChange(change) => {
                        if change.original.trim_end_matches('.')
                            == fqdn(&self.host).trim_end_matches('.')
                        {
                            renamed = Some(change.new_name);
                        } else {
                            // A renamed instance ("Show Master (2)") is still our own.
                            for r in [&mut self.http, &mut self.rtp].into_iter().flatten() {
                                if r.fullname.eq_ignore_ascii_case(&change.original) {
                                    info!(from = %change.original, to = %change.new_name, "mDNS name taken on the network - renamed");
                                    r.fullname = change.new_name.clone();
                                }
                            }
                        }
                    }
                    DaemonEvent::Error(e) => {
                        debug!("mDNS daemon: {e}");
                        self.error = Some(e.to_string());
                    }
                    _ => {}
                }
            }
        }
        if let Some(name) = renamed {
            let label = host_label(name.trim_end_matches('.').trim_end_matches(".local"));
            if label != self.host {
                info!(from = %self.host, to = %label, "mDNS host name taken on the network - renamed");
            }
            self.host = label;
            if let (Some(url), Some(http)) = (&mut self.url, &self.http) {
                *url = format!("http://{}.local:{}", self.host, http.key.1);
            }
        }
    }

    /// Current state for display.
    pub fn status(&self) -> Status {
        let own = self.rtp.as_ref().map(|r| r.fullname.to_lowercase());
        Status {
            active: self.daemon.is_some(),
            host: format!("{}.local", self.host),
            url: self.url.clone(),
            rtp_advertised: self.rtp.is_some(),
            rtp_instance: self
                .rtp
                .as_ref()
                .map(|r| instance_of(&r.fullname, APPLE_MIDI_SERVICE)),
            error: self.error.clone(),
            discovered: {
                let mut list: Vec<Discovered> = self
                    .found
                    .iter()
                    .filter(|(full, _)| own.as_deref() != Some(full.as_str()))
                    .map(|(_, d)| d.clone())
                    .collect();
                list.sort_by(|a, b| a.name.cmp(&b.name).then(a.addr.cmp(&b.addr)));
                list
            },
        }
    }

    /// Withdraws all advertisements (goodbye packets) and stops the daemon.
    /// Does not wait for the daemon thread.
    pub fn shutdown(&mut self) {
        if let Some(daemon) = self.daemon.take() {
            for r in [self.http.take(), self.rtp.take()].into_iter().flatten() {
                let _ = daemon.unregister(&r.fullname);
            }
            if self.browse.take().is_some() {
                let _ = daemon.stop_browse(APPLE_MIDI_SERVICE);
            }
            let _ = daemon.shutdown();
            info!("mDNS stopped");
        }
        self.monitor = None;
        self.found.clear();
        self.url = None;
    }

    fn ensure_daemon(&mut self) -> Option<ServiceDaemon> {
        if let Some(d) = &self.daemon {
            return Some(d.clone());
        }
        if self.retry_at.is_some_and(|t| Instant::now() < t) {
            return None;
        }
        match start_daemon(self.port) {
            Ok(d) => {
                info!(port = self.port, host = %self.host, "mDNS started (local network only)");
                self.monitor = d.monitor().ok();
                self.retry_at = None;
                if let Some(problem) = multicast_problem() {
                    warn!("mDNS: {problem} - devices may not find this computer by name; use the IP address");
                    self.error = Some(problem);
                } else {
                    self.error = None;
                }
                self.daemon = Some(d.clone());
                Some(d)
            }
            Err(e) => {
                warn!("mDNS unavailable: {e} - use the IP address / QR code instead");
                self.error = Some(e);
                self.retry_at = Some(Instant::now() + RETRY);
                None
            }
        }
    }

    fn note(&mut self, result: Result<(), mdns_sd::Error>, what: &str) {
        if let Err(e) = result {
            warn!("mDNS: cannot advertise {what}: {e}");
            self.error = Some(format!("{what}: {e}"));
        }
    }

    fn on_service_event(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::ServiceResolved(service) => {
                let fullname = service.get_fullname().to_lowercase();
                let addrs = service.get_addresses_v4();
                let Some(ip) = pick_ipv4(addrs) else {
                    return;
                };
                let entry = Discovered {
                    name: instance_of(service.get_fullname(), APPLE_MIDI_SERVICE),
                    addr: std::net::SocketAddr::new(IpAddr::V4(ip), service.get_port()).to_string(),
                    host: service.get_hostname().trim_end_matches('.').to_owned(),
                };
                if self.found.get(&fullname) != Some(&entry) {
                    debug!(name = %entry.name, addr = %entry.addr, "RTP-MIDI session found");
                }
                self.found.insert(fullname, entry);
            }
            ServiceEvent::ServiceRemoved(_, fullname) => {
                self.found.remove(&fullname.to_lowercase());
            }
            _ => {}
        }
    }
}

impl Drop for Mdns {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn start_daemon(port: u16) -> Result<ServiceDaemon, String> {
    ServiceDaemon::new_with_port(port).map_err(|e| e.to_string())
}

/// Checks whether this machine can join the mDNS multicast group at all
/// (opens an ephemeral socket; sends nothing). `None` = fine.
pub fn multicast_problem() -> Option<String> {
    let socket = match std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        Ok(s) => s,
        Err(e) => return Some(format!("cannot open a UDP socket: {e}")),
    };
    socket
        .join_multicast_v4(&Ipv4Addr::new(224, 0, 0, 251), &Ipv4Addr::UNSPECIFIED)
        .err()
        .map(|e| format!("no multicast on this network ({e})"))
}

/// Registers, re-registers or withdraws one service so that it matches `want`.
fn sync_service(
    daemon: &ServiceDaemon,
    slot: &mut Option<Registered>,
    want: Option<(String, u16)>,
    build: impl FnOnce(&(String, u16)) -> Result<ServiceInfo, mdns_sd::Error>,
) -> Result<(), mdns_sd::Error> {
    if slot.as_ref().map(|r| &r.key) == want.as_ref() {
        return Ok(());
    }
    let replaced = slot.take().map(|old| {
        let _ = daemon.unregister(&old.fullname);
        old.fullname
    });
    let Some(key) = want else {
        return Ok(());
    };
    let mut info = build(&key)?;
    let fullname = info.get_fullname().to_owned();
    // Re-announcing a name we already own (e.g. only the port changed): no new
    // probe, otherwise our own goodbye for the old record looks like a conflict.
    if replaced.is_some_and(|old| old.eq_ignore_ascii_case(&fullname)) {
        info.set_requires_probe(false);
    }
    daemon.register(info)?;
    info!(service = %fullname, port = key.1, "mDNS: advertising");
    *slot = Some(Registered { key, fullname });
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn host_labels() {
        assert_eq!(host_label("showmaster"), "showmaster");
        assert_eq!(host_label("Studio PC (FOH)"), "studio-pc-foh");
        assert_eq!(host_label("--Ünïcode--"), "n-code");
        assert_eq!(host_label(""), "showmaster");
        assert_eq!(host_label("!!!"), "showmaster");
        assert_eq!(host_label(&"a".repeat(100)).len(), 63);
        assert!(!host_label(&format!("{}-b", "a".repeat(62))).ends_with('-'));
    }

    #[test]
    fn instance_names() {
        assert_eq!(http_instance("studio-mac"), "Show Master on studio-mac");
        assert_eq!(
            http_instance("studio-mac.local"),
            "Show Master on studio-mac"
        );
        assert_eq!(http_instance("  "), "Show Master");
        let long = http_instance(&"ä".repeat(60));
        assert!(long.len() <= 63);
        assert!(long.starts_with("Show Master on ä"));
        assert_eq!(instance_label(""), "Show Master");
        assert_eq!(instance_label(" Main Rig "), "Main Rig");
    }

    #[test]
    fn service_records() {
        let http = http_service("Show Master on foh", "showmaster", 7400).unwrap();
        assert_eq!(http.get_fullname(), "Show Master on foh._http._tcp.local.");
        assert_eq!(http.get_hostname(), "showmaster.local.");
        assert_eq!(http.get_port(), 7400);
        assert_eq!(http.get_property_val_str("path"), Some("/remote"));
        assert!(http.is_addr_auto());

        let rtp = rtp_service(
            &RtpAdvert {
                name: "Show Master".into(),
                port: 5004,
            },
            "showmaster.local",
        )
        .unwrap();
        assert_eq!(rtp.get_type(), APPLE_MIDI_SERVICE);
        assert_eq!(rtp.get_fullname(), "Show Master._apple-midi._udp.local.");
        assert_eq!(rtp.get_hostname(), "showmaster.local.");
        assert_eq!(rtp.get_port(), 5004);
    }

    #[test]
    fn instance_names_are_unescaped() {
        assert_eq!(
            instance_of("Mac Studio._apple-midi._udp.local.", APPLE_MIDI_SERVICE),
            "Mac Studio"
        );
        assert_eq!(
            instance_of("A\\.B._apple-midi._udp.local.", APPLE_MIDI_SERVICE),
            "A.B"
        );
        assert_eq!(
            instance_of("Caf\\195\\169._apple-midi._udp.local.", APPLE_MIDI_SERVICE),
            "Café"
        );
        assert_eq!(instance_of("odd", APPLE_MIDI_SERVICE), "odd");
    }

    #[test]
    fn address_choice() {
        let lo = Ipv4Addr::LOCALHOST;
        let ll = Ipv4Addr::new(169, 254, 3, 4);
        let lan = Ipv4Addr::new(192, 168, 1, 20);
        assert_eq!(pick_ipv4([lo, ll, lan]), Some(lan));
        assert_eq!(pick_ipv4([lo, ll]), Some(ll));
        assert_eq!(pick_ipv4([lo]), Some(lo));
        assert_eq!(pick_ipv4([]), None);
    }

    #[test]
    fn idle_means_no_daemon() {
        let mut m = Mdns::with_options(free_port(), "test-idle", "box");
        m.apply(&Desired::default());
        assert!(!m.status().active);
        assert_eq!(m.status().host, "test-idle.local");
        m.poll();
        m.shutdown();
    }

    fn free_port() -> u16 {
        std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .and_then(|s| s.local_addr())
            .map(|a| a.port())
            .unwrap_or(15353)
    }

    /// Port already taken exclusively by another socket: the daemon cannot
    /// open its multicast sockets. Nothing may panic or block; turning it off
    /// again works.
    #[test]
    fn port_in_use_fails_gracefully() {
        let blocker = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let port = blocker.local_addr().unwrap().port();
        let mut m = Mdns::with_options(port, "test-busy", "box");
        let started = Instant::now();
        m.apply(&Desired {
            http_port: Some(7400),
            rtp: Some(RtpAdvert {
                name: "Busy".into(),
                port: 5004,
            }),
            browse_rtp: true,
        });
        for _ in 0..5 {
            m.poll();
            std::thread::sleep(Duration::from_millis(20));
        }
        let status = m.status();
        assert!(status.discovered.is_empty());
        assert_eq!(status.url.as_deref(), Some("http://test-busy.local:7400"));
        m.apply(&Desired::default());
        assert!(!m.status().active);
        assert!(started.elapsed() < Duration::from_secs(2), "must not block");
    }

    /// A daemon that cannot be started is retried later, not on every call.
    #[test]
    fn start_failure_is_reported_and_retried_later() {
        let mut m = Mdns::with_options(free_port(), "test-fail", "box");
        m.error = Some("simulated".into());
        m.retry_at = Some(Instant::now() + RETRY);
        m.apply(&Desired {
            http_port: Some(7400),
            ..Desired::default()
        });
        let status = m.status();
        assert!(!status.active);
        assert_eq!(status.url, None);
        assert_eq!(status.error.as_deref(), Some("simulated"));
    }
}
