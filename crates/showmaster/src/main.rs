//! Show Master: playback and show control.
//!
//! This binary runs the complete application: cue engine, audio/MIDI/
//! timecode I/O and the embedded web server that serves the user interface
//! (desktop UI on this machine, remote UI for tablets on the local network).
//! It never contacts the internet.

use sm_server::runtime::{self, Config};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

const USAGE: &str = "\
Show Master - playback and show control

USAGE:
    showmaster [OPTIONS] [SHOWFILE.smshow]

OPTIONS:
    --port <N>        HTTP port (default: the show's remote port, 7400)
    --local-only      Listen on 127.0.0.1 only, whatever the show says
    --log-dir <DIR>   Directory for log files
    --open            Open the user interface in the default browser
    -h, --help        Show this help
    -V, --version     Show the version
";

struct Args {
    show: Option<PathBuf>,
    port: Option<u16>,
    local_only: bool,
    log_dir: Option<PathBuf>,
    open: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        show: None,
        port: None,
        local_only: false,
        log_dir: None,
        open: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("showmaster {}", runtime::VERSION);
                std::process::exit(0);
            }
            "--port" => {
                args.port = Some(
                    it.next()
                        .and_then(|p| p.parse().ok())
                        .ok_or("--port needs a number")?,
                )
            }
            "--local-only" => args.local_only = true,
            "--open" => args.open = true,
            "--log-dir" => {
                args.log_dir = Some(
                    it.next()
                        .map(PathBuf::from)
                        .ok_or("--log-dir needs a path")?,
                )
            }
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other => args.show = Some(PathBuf::from(other)),
        }
    }
    Ok(args)
}

/// Per-platform directory for logs.
fn default_log_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("ShowMaster")
            .join("logs")
    } else if cfg!(target_os = "macos") {
        home.join("Library").join("Logs").join("ShowMaster")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local").join("state"))
            .join("showmaster")
    }
}

#[derive(Clone)]
struct Tee(Arc<Mutex<Option<File>>>);

struct TeeWriter(Arc<Mutex<Option<File>>>);

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        if let Ok(mut guard) = self.0.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.write_all(buf);
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Tee {
    type Writer = TeeWriter;
    fn make_writer(&'a self) -> Self::Writer {
        TeeWriter(self.0.clone())
    }
}

fn init_logging(dir: PathBuf) -> PathBuf {
    let path = dir.join("showmaster.log");
    let file = fs::create_dir_all(&dir).ok().and_then(|_| {
        // Keep one previous log.
        if fs::metadata(&path)
            .map(|m| m.len() > 5_000_000)
            .unwrap_or(false)
        {
            let _ = fs::rename(&path, dir.join("showmaster.old.log"));
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
    });
    let tee = Tee(Arc::new(Mutex::new(file)));
    let filter = tracing_subscriber::EnvFilter::try_from_env("SHOWMASTER_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(tee)
        .try_init();
    // A panic anywhere is logged with its location; the audio callback and the
    // engine thread additionally contain panics so the show keeps running.
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        tracing::error!(%location, %thread, "PANIC: {message}");
    }));
    path
}

fn open_browser(url: &str) {
    let result = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = result {
        tracing::warn!("cannot open browser: {e}");
    }
}

async fn bind(addr: IpAddr, port: u16) -> std::io::Result<tokio::net::TcpListener> {
    match tokio::net::TcpListener::bind(SocketAddr::new(addr, port)).await {
        Ok(l) => Ok(l),
        Err(e) => {
            tracing::error!("cannot listen on port {port}: {e} - falling back to a free port on this machine only");
            tokio::net::TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).await
        }
    }
}

fn main() -> std::process::ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return std::process::ExitCode::from(2);
        }
    };
    let log_path = init_logging(args.log_dir.clone().unwrap_or_else(default_log_dir));
    tracing::info!(
        "Show Master {} starting (log: {})",
        runtime::VERSION,
        log_path.display()
    );

    let tokio_rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("sm-net")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("cannot start network runtime: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let (handle, engine_thread) = runtime::spawn(Config {
        show_path: args.show.clone(),
        port: args.port,
        ..Config::default()
    });
    let state = handle.state();
    let remote = &state["show"]["settings"]["remote"];
    let port = args
        .port
        .or_else(|| remote["port"].as_u64().map(|p| p as u16))
        .unwrap_or(7400);
    let lan = remote["enabled"].as_bool().unwrap_or(false) && !args.local_only;
    let osc_port = remote["osc_port"].as_u64().unwrap_or(0) as u16;
    let addr = if lan {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    };

    if osc_port != 0 {
        match std::net::UdpSocket::bind(SocketAddr::new(addr, osc_port)) {
            Ok(socket) => {
                let h = handle.clone();
                tracing::info!("OSC listening on UDP {osc_port}");
                if let Err(e) = std::thread::Builder::new()
                    .name("sm-osc".into())
                    .spawn(move || sm_server::osc::listen(socket, h))
                {
                    tracing::error!("cannot start OSC thread: {e}");
                }
            }
            Err(e) => tracing::error!("cannot listen for OSC on UDP {osc_port}: {e}"),
        }
    }

    let open = args.open;
    let server_handle = handle.clone();
    let code = tokio_rt.block_on(async move {
        let listener = match bind(addr, port).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("cannot open any port: {e}");
                return std::process::ExitCode::FAILURE;
            }
        };
        let local = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        let url = format!("http://127.0.0.1:{local}");
        tracing::info!("user interface: {url}");
        if lan {
            for u in sm_server::http::local_urls(local) {
                tracing::info!("remote (tablet): {u}/remote   view only: {u}/view");
            }
        }
        println!("Show Master is running. Open {url}");
        if open {
            open_browser(&url);
        }
        tokio::select! {
            r = sm_server::http::serve(server_handle, listener) => {
                if let Err(e) = r {
                    tracing::error!("server stopped: {e}");
                }
                std::process::ExitCode::FAILURE
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down");
                std::process::ExitCode::SUCCESS
            }
        }
    });
    // Dropping the last handle stops the engine thread, which autosaves on the way out.
    drop(handle);
    tokio_rt.shutdown_timeout(std::time::Duration::from_secs(1));
    let _ = engine_thread.join();
    code
}
