//! Connection of the window to the runtime: reads the published state and
//! sends commands over the same command bus that hotkeys, tablets and the
//! Stream Deck API use. Never blocks the UI thread.

use crate::model::{Tick, UiState};
use serde_json::Value;
use sm_server::runtime::{parse_command, Handle, LogEntry, Role};
use std::sync::mpsc;

pub type Reply = (u64, Result<Value, String>);

pub struct Client {
    pub handle: Handle,
    rt: tokio::runtime::Handle,
    rev: Option<u64>,
    pub state: UiState,
    pub tick: Tick,
    pub log: Vec<LogEntry>,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
    next_id: u64,
    repaint: Option<egui::Context>,
}

impl Client {
    pub fn new(handle: Handle, rt: tokio::runtime::Handle) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut c = Self {
            handle,
            rt,
            rev: None,
            state: UiState::default(),
            tick: Tick::default(),
            log: Vec::new(),
            tx,
            rx,
            next_id: 1,
            repaint: None,
        };
        c.refresh();
        c
    }

    pub fn set_repaint_context(&mut self, ctx: egui::Context) {
        self.repaint = Some(ctx);
    }

    /// Pull the latest published state. Returns true when the document changed.
    pub fn refresh(&mut self) -> bool {
        let Ok(p) = self.handle.published.read() else {
            return false;
        };
        let changed = self.rev != Some(p.rev);
        if changed {
            match serde_json::from_value::<UiState>(p.state.clone()) {
                Ok(s) => self.state = s,
                Err(e) => tracing::error!("cannot read state document: {e}"),
            }
            self.rev = Some(p.rev);
        }
        if let Ok(t) = serde_json::from_value::<Tick>(p.tick.clone()) {
            self.tick = t;
        }
        if p.log_tail.len() != self.log.len()
            || p.log_tail.last().map(|e| e.time_ms) != self.log.last().map(|e| e.time_ms)
        {
            self.log = p.log_tail.clone();
        }
        changed
    }

    /// Send a command; the result arrives later through [`Client::poll`].
    pub fn send(&mut self, cmd: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        match parse_command(cmd) {
            Ok(command) => {
                let handle = self.handle.clone();
                let tx = self.tx.clone();
                let repaint = self.repaint.clone();
                self.rt.spawn(async move {
                    let result = handle.command(command, Role::Admin, "desktop").await;
                    let _ = tx.send((id, result));
                    if let Some(ctx) = repaint {
                        ctx.request_repaint();
                    }
                });
            }
            Err(e) => {
                let _ = self.tx.send((id, Err(e)));
            }
        }
        id
    }

    /// Command results that arrived since the last call.
    pub fn poll(&mut self) -> Vec<Reply> {
        self.rx.try_iter().collect()
    }
}
