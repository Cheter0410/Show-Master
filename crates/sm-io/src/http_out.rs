//! Sender for HTTP cues: requests wait for their due instant on a worker
//! thread, then each one goes out on its own short-lived thread (a slow or
//! unreachable device never delays the next request, let alone the show).
//! Only the status line of the answer is read, for the log.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const IO_TIMEOUT: Duration = Duration::from_secs(3);

struct Job {
    due: Instant,
    addr: String,
    bytes: Vec<u8>,
    summary: String,
}

type Log = Arc<Mutex<Vec<String>>>;

pub struct HttpOut {
    tx: mpsc::Sender<Job>,
    log: Log,
}

impl Default for HttpOut {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpOut {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        let _ = std::thread::Builder::new()
            .name("sm-http".into())
            .spawn(move || run(rx, l));
        Self { tx, log }
    }

    pub fn send_at(&self, due: Instant, addr: String, bytes: Vec<u8>, summary: String) {
        let _ = self.tx.send(Job {
            due,
            addr,
            bytes,
            summary,
        });
    }

    /// Failures since the last call (successes only go to the log file).
    pub fn take_errors(&self) -> Vec<String> {
        self.log
            .lock()
            .map(|mut l| std::mem::take(&mut *l))
            .unwrap_or_default()
    }
}

fn run(rx: mpsc::Receiver<Job>, log: Log) {
    let mut queue: Vec<Job> = Vec::new();
    loop {
        let now = Instant::now();
        let (due, later): (Vec<Job>, Vec<Job>) = queue.drain(..).partition(|j| j.due <= now);
        queue = later;
        for job in due {
            let log = log.clone();
            let _ = std::thread::Builder::new()
                .name("sm-http-req".into())
                .spawn(move || match request(&job.addr, &job.bytes) {
                    Ok(status) => tracing::info!("HTTP cue {}: {status}", job.summary),
                    Err(e) => {
                        if let Ok(mut l) = log.lock() {
                            if l.len() < 64 {
                                l.push(format!("HTTP cue {} failed: {e}", job.summary));
                            }
                        }
                    }
                });
        }
        let wait = queue
            .iter()
            .map(|j| j.due.saturating_duration_since(Instant::now()))
            .min()
            .unwrap_or(Duration::from_millis(500));
        match rx.recv_timeout(wait) {
            Ok(job) => queue.push(job),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Send the request, return the answer's status line.
fn request(addr: &str, bytes: &[u8]) -> Result<String, String> {
    let sock = addr
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("address not found")?;
    let mut s = TcpStream::connect_timeout(&sock, CONNECT_TIMEOUT).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    s.write_all(bytes).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 256];
    let mut got = Vec::new();
    while !got.contains(&b'\n') && got.len() < 256 {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(e) => return Err(format!("no answer ({e})")),
        }
    }
    let line = String::from_utf8_lossy(&got);
    let line = line.lines().next().unwrap_or("").trim().to_string();
    if line.is_empty() {
        return Err("empty answer".into());
    }
    let code = line.split_whitespace().nth(1).unwrap_or("");
    if code.starts_with('2') || code.starts_with('3') {
        Ok(line)
    } else {
        Err(line)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn sends_on_time_and_reports_the_status() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap().to_string();
        let got = std::thread::spawn(move || {
            let (mut c, _) = server.accept().unwrap();
            let at = Instant::now();
            let mut buf = vec![0u8; 1024];
            let n = c.read(&mut buf).unwrap();
            c.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").unwrap();
            (at, String::from_utf8_lossy(&buf[..n]).to_string())
        });
        let out = HttpOut::new();
        let due = Instant::now() + Duration::from_millis(50);
        out.send_at(
            due,
            addr,
            b"GET /x HTTP/1.1\r\nHost: h\r\n\r\n".to_vec(),
            "GET http://h/x".into(),
        );
        let (at, req) = got.join().unwrap();
        assert!(at >= due);
        assert!(req.starts_with("GET /x"));
        std::thread::sleep(Duration::from_millis(100));
        assert!(out.take_errors().is_empty());
    }

    #[test]
    fn refused_connection_is_reported() {
        // A port nothing listens on.
        let free = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let out = HttpOut::new();
        out.send_at(
            Instant::now(),
            free.to_string(),
            b"GET / HTTP/1.1\r\n\r\n".to_vec(),
            "GET x".into(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(msg) = out.take_errors().first() {
                assert!(msg.contains("failed"), "{msg}");
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
