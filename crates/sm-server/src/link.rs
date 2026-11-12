//! Backup computer (hot standby): the main computer mirrors everything to a
//! second Show Master over the show network, so the backup runs the same show
//! in parallel and can take over (audio switcher / second PA feed) at once.
//!
//! * Per machine (`link.json` next to the other settings), not in the show
//!   file: the same show runs on both computers with different roles.
//! * Main -> backup only: every transport command the main executes (GO,
//!   Stop, PANIC, playhead, timecode, ...) is forwarded right after it ran
//!   locally; the show document and the mode are synced when they change
//!   (and in full after every (re)connect, including the playheads).
//! * Plain HTTP `POST /api/link` on the backup's web port with a shared
//!   key (`X-ShowMaster-Link`). Commands that cannot be delivered are
//!   dropped, never replayed late: a GO sent seconds later would be wrong.
//! * The backup shows when the main was last heard of; when the main stops
//!   (crash, power, cable) the backup simply keeps running on its own.
//!
//! Media must exist on the backup at the same place relative to the show
//! (copy the show folder or a show package there and open it once).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const HEADER: &str = "x-showmaster-link";
const TAKEN_OVER: &str = "the backup has taken over - this computer is no longer mirrored";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LinkRole {
    #[default]
    Off,
    Main,
    Backup,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct LinkConfig {
    pub role: LinkRole,
    /// Main: the backup's address, `host` or `host:port` (web port, default 7400).
    pub peer: String,
    /// Shared key, the same on both computers.
    pub key: String,
    /// Backup: take over by itself when the main is silent for
    /// [`LOST_AFTER`] (after it has been heard once).
    #[serde(default = "yes")]
    pub auto_takeover: bool,
}

fn yes() -> bool {
    true
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            role: LinkRole::Off,
            peer: String::new(),
            key: String::new(),
            auto_takeover: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Status {
    connected: bool,
    last_ok_ms: Option<u64>,
    error: Option<String>,
    /// Backup: when the main was last heard of.
    last_seen: Option<Instant>,
    last_seen_ms: Option<u64>,
    /// Main: the backup said it has taken over.
    backup_took_over: bool,
    /// Backup: web port of the main (for tablets that fail over back).
    main_addr: Option<String>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

const PING_EVERY: Duration = Duration::from_millis(400);
/// The backup reports the main as lost (and takes over, if set) after this
/// long without a message.
pub const LOST_AFTER: Duration = Duration::from_millis(2000);
const SYNC_CHECK: Duration = Duration::from_millis(500);

pub struct Link {
    file: Option<PathBuf>,
    config: LinkConfig,
    status: Arc<Mutex<Status>>,
    tx: Option<mpsc::Sender<Value>>,
    /// Set by the sender after a (re)connect: send everything again.
    need_full: Arc<AtomicBool>,
    last_check: Option<Instant>,
    last_hash: Option<u64>,
    last_mode: Option<String>,
    /// Backup: running on its own (taken over), not following the main.
    taken_over: bool,
}

impl Link {
    pub fn load(file: Option<PathBuf>) -> Self {
        let config = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<LinkConfig>(&t).ok())
            .unwrap_or_default();
        let mut l = Self {
            file,
            config: LinkConfig::default(),
            status: Arc::default(),
            tx: None,
            need_full: Arc::new(AtomicBool::new(true)),
            last_check: None,
            last_hash: None,
            last_mode: None,
            taken_over: false,
        };
        l.apply(config);
        l
    }

    pub fn config(&self) -> &LinkConfig {
        &self.config
    }

    pub fn role(&self) -> LinkRole {
        self.config.role
    }

    /// Change the configuration (saved) and (re)start the sender.
    pub fn set(&mut self, config: LinkConfig) -> Result<(), String> {
        if config.role != LinkRole::Off && config.key.trim().len() < 4 {
            return Err("the link key needs at least 4 characters".into());
        }
        if config.role == LinkRole::Main && config.peer.trim().is_empty() {
            return Err("enter the backup computer's address".into());
        }
        self.apply(config);
        if let Some(file) = &self.file {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let text = serde_json::to_string_pretty(&self.config).map_err(|e| e.to_string())?;
            std::fs::write(file, text).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn apply(&mut self, config: LinkConfig) {
        self.tx = None; // stops the old sender (its channel closes)
        if let Ok(mut s) = self.status.lock() {
            *s = Status::default();
        }
        self.need_full.store(true, Ordering::SeqCst);
        self.last_hash = None;
        self.last_mode = None;
        self.taken_over = false;
        if config.role == LinkRole::Main && !config.peer.trim().is_empty() {
            let (tx, rx) = mpsc::channel();
            let (peer, key) = (config.peer.trim().to_string(), config.key.clone());
            let (status, need_full) = (self.status.clone(), self.need_full.clone());
            let _ = std::thread::Builder::new()
                .name("sm-link".into())
                .spawn(move || sender(rx, &peer, &key, &status, &need_full));
            self.tx = Some(tx);
        }
        self.config = config;
    }

    /// Main: a transport command ran here; forward it.
    pub fn forward(&self, cmd: Value) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(json!({ "kind": "cmd", "cmd": cmd }));
        }
    }

    /// Main: is a (cheap) check of show / mode due? Avoids serializing the
    /// show on every pass of the engine loop.
    pub fn sync_due(&self) -> bool {
        self.tx.is_some()
            && (self.need_full.load(Ordering::SeqCst)
                || self.last_check.is_none_or(|t| t.elapsed() >= SYNC_CHECK))
    }

    /// Main: called often; sends the show / mode when they changed (checked
    /// twice a second) and everything after a (re)connect.
    pub fn maybe_sync(&mut self, show: &Value, mode: &str, playheads: &Value, auth: &Value) {
        let Some(tx) = &self.tx else { return };
        let full = self.need_full.load(Ordering::SeqCst);
        if !full && self.last_check.is_some_and(|t| t.elapsed() < SYNC_CHECK) {
            return;
        }
        self.last_check = Some(Instant::now());
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            show.to_string().hash(&mut h);
            auth.to_string().hash(&mut h);
            h.finish()
        };
        if !full && self.last_hash == Some(hash) && self.last_mode.as_deref() == Some(mode) {
            return;
        }
        self.need_full.store(false, Ordering::SeqCst);
        self.last_hash = Some(hash);
        self.last_mode = Some(mode.to_string());
        let _ = tx.send(json!({
            "kind": "sync", "show": show, "mode": mode, "playheads": playheads, "initial": full,
            "auth": auth,
        }));
    }

    /// Backup: is this request from our main (right key)?
    pub fn accepts(&self, key: &str) -> bool {
        self.config.role == LinkRole::Backup
            && self.config.key.len() >= 4
            && crate::http::token_eq(&self.config.key, key)
    }

    /// Backup that follows the main: its own outputs (MIDI, OSC, HTTP,
    /// timecode) are muted and local transport commands are refused, so
    /// nothing happens twice. Audio keeps playing for a seamless switch.
    pub fn standby(&self) -> bool {
        self.config.role == LinkRole::Backup && !self.taken_over
    }

    pub fn taken_over(&self) -> bool {
        self.taken_over
    }

    /// Backup: run on its own from now on (button, or the main went silent).
    pub fn take_over(&mut self) {
        if self.config.role == LinkRole::Backup {
            self.taken_over = true;
        }
    }

    /// Backup: follow the main again (it sends everything when it is back).
    pub fn follow_main(&mut self) {
        self.taken_over = false;
        if let Ok(mut s) = self.status.lock() {
            s.last_seen = None;
        }
    }

    /// Backup, called often: take over when the main, once heard, has been
    /// silent for [`LOST_AFTER`]. Returns true at that moment.
    pub fn check_takeover(&mut self) -> bool {
        if self.config.role != LinkRole::Backup || self.taken_over || !self.config.auto_takeover {
            return false;
        }
        let lost = self
            .status
            .lock()
            .map(|s| s.last_seen.is_some_and(|t| t.elapsed() >= LOST_AFTER))
            .unwrap_or(false);
        if lost {
            self.taken_over = true;
        }
        lost
    }

    /// Backup: the main's web address (from its messages), for tablets.
    pub fn set_main_addr(&self, addr: String) {
        if let Ok(mut s) = self.status.lock() {
            s.main_addr = Some(addr);
        }
    }

    /// Where tablets go when this computer stops answering: the main points
    /// them at the backup.
    pub fn failover_url(&self) -> Option<String> {
        match self.config.role {
            LinkRole::Main if !self.config.peer.trim().is_empty() => {
                let p = self
                    .config
                    .peer
                    .trim()
                    .trim_start_matches("http://")
                    .trim_end_matches('/');
                Some(format!("http://{p}"))
            }
            _ => None,
        }
    }

    /// Backup: a message from the main arrived.
    pub fn seen(&self) {
        if let Ok(mut s) = self.status.lock() {
            s.last_seen = Some(Instant::now());
            s.last_seen_ms = Some(now_ms());
        }
    }

    /// For the state document (`link`); never contains the key.
    pub fn status_json(&self) -> Value {
        let s = self.status.lock().map(|s| s.clone()).unwrap_or_default();
        let main_alive = s.last_seen.is_some_and(|t| t.elapsed() < LOST_AFTER);
        json!({
            "role": self.config.role,
            "standby": self.standby(),
            "taken_over": self.taken_over,
            "backup_took_over": s.backup_took_over,
            "auto_takeover": self.config.auto_takeover,
            "failover_url": self.failover_url(),
            "peer": self.config.peer,
            "has_key": !self.config.key.is_empty(),
            "connected": match self.config.role {
                LinkRole::Main => s.connected,
                LinkRole::Backup => main_alive,
                LinkRole::Off => false,
            },
            "last_ok_ms": s.last_ok_ms,
            "last_seen_ms": s.last_seen_ms,
            "error": s.error,
        })
    }
}

/// `host` / `host:port` -> socket address (web port 7400 by default).
fn peer_addr(peer: &str) -> Result<std::net::SocketAddr, String> {
    let p = peer
        .trim()
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let with_port = if p
        .rsplit_once(':')
        .is_some_and(|(_, port)| port.parse::<u16>().is_ok())
    {
        p.to_string()
    } else {
        format!("{p}:7400")
    };
    with_port
        .to_socket_addrs()
        .map_err(|e| format!("{peer}: {e}"))?
        .next()
        .ok_or_else(|| format!("{peer}: address not found"))
}

fn post(addr: std::net::SocketAddr, key: &str, body: &str) -> Result<(), String> {
    let mut s =
        TcpStream::connect_timeout(&addr, Duration::from_millis(800)).map_err(|e| e.to_string())?;
    let _ = s.set_nodelay(true);
    s.set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    let head = format!(
        "POST /api/link HTTP/1.1\r\nHost: {addr}\r\n{HEADER}: {key}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    s.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).map_err(|e| e.to_string())?;
    let line = String::from_utf8_lossy(&buf[..n]);
    match line.split_whitespace().nth(1) {
        Some("200") => Ok(()),
        Some("401") => Err("the backup refused the link key (or is not set to Backup)".into()),
        Some("409") => Err(TAKEN_OVER.into()),
        Some(code) => Err(format!("backup answered {code}")),
        None => Err("no answer".into()),
    }
}

fn sender(
    rx: mpsc::Receiver<Value>,
    peer: &str,
    key: &str,
    status: &Mutex<Status>,
    need_full: &AtomicBool,
) {
    let mut last_send = Instant::now() - PING_EVERY;
    loop {
        let wait = PING_EVERY.saturating_sub(last_send.elapsed());
        let msg = match rx.recv_timeout(wait) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => json!({ "kind": "ping" }),
            Err(RecvTimeoutError::Disconnected) => return,
        };
        last_send = Instant::now();
        let result = peer_addr(peer).and_then(|a| post(a, key, &msg.to_string()));
        let was = status.lock().map(|s| s.connected).unwrap_or(false);
        if let Ok(mut s) = status.lock() {
            match &result {
                Ok(()) => {
                    s.connected = true;
                    s.backup_took_over = false;
                    s.last_ok_ms = Some(now_ms());
                    s.error = None;
                }
                Err(e) => {
                    s.connected = false;
                    s.backup_took_over = e == TAKEN_OVER;
                    s.error = Some(e.clone());
                }
            }
        }
        match result {
            // Back in touch: the runtime sends the whole state again.
            Ok(()) if !was => {
                need_full.store(true, Ordering::SeqCst);
                tracing::info!("backup link to {peer} connected");
            }
            Err(e) if was => tracing::warn!("backup link to {peer} lost: {e}"),
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn config_rules_and_persistence() {
        let dir = std::env::temp_dir().join(format!("sm-link-{}", std::process::id()));
        let file = dir.join("link.json");
        let mut l = Link::load(Some(file.clone()));
        assert_eq!(l.role(), LinkRole::Off);
        assert!(l
            .set(LinkConfig {
                role: LinkRole::Backup,
                peer: String::new(),
                key: "ab".into(),
                auto_takeover: true,
            })
            .is_err());
        assert!(l
            .set(LinkConfig {
                role: LinkRole::Main,
                peer: String::new(),
                key: "abcd".into(),
                auto_takeover: true,
            })
            .is_err());
        l.set(LinkConfig {
            role: LinkRole::Backup,
            peer: String::new(),
            key: "secret".into(),
            auto_takeover: true,
        })
        .unwrap();
        let again = Link::load(Some(file));
        assert_eq!(again.role(), LinkRole::Backup);
        assert!(again.accepts("secret"));
        assert!(!again.accepts("secreT"));
        assert!(!again.status_json().to_string().contains("secret"));
        assert_eq!(again.status_json()["connected"], false);
        again.seen();
        assert_eq!(again.status_json()["connected"], true);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn peer_addresses() {
        assert_eq!(peer_addr("127.0.0.1").unwrap().port(), 7400);
        assert_eq!(peer_addr("http://127.0.0.1:7500/").unwrap().port(), 7500);
        assert!(peer_addr("no-such-host.invalid").is_err());
    }
}
