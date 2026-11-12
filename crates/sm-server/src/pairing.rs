//! Paired devices: a tablet scans a QR code that carries a long random
//! token and is logged in from then on - no PIN to type. Pairings are per
//! machine (not in the show file), kept in `pairings.json` next to the other
//! settings, and can be revoked one by one or all at once.
//!
//! The token is only ever shown inside the QR code right after pairing; the
//! state document lists devices without tokens.

use crate::runtime::Role;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pairing {
    pub id: u64,
    pub token: String,
    pub role: Role,
    pub name: String,
    pub created_ms: u64,
}

#[derive(Default)]
pub struct Pairings {
    file: Option<PathBuf>,
    list: Vec<Pairing>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Pairings {
    /// Load from `file` (missing or broken file = no pairings).
    pub fn load(file: Option<PathBuf>) -> Self {
        let list = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<Vec<Pairing>>(&t).ok())
            .unwrap_or_default();
        Self { file, list }
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(text) = serde_json::to_string_pretty(&self.list) else {
            return;
        };
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }

    /// Pair a new device; the result carries the token for the QR code.
    pub fn create(&mut self, role: Role, name: &str) -> Pairing {
        let id = self.list.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        let name = name.trim();
        let p = Pairing {
            id,
            token: crate::http::random_token(),
            role: if role == Role::Admin {
                Role::Control
            } else {
                role
            },
            name: if name.is_empty() {
                format!("Device {id}")
            } else {
                name.chars().take(40).collect()
            },
            created_ms: now_ms(),
        };
        self.list.push(p.clone());
        self.save();
        p
    }

    /// Remove one pairing; returns its token (to disconnect its clients).
    pub fn revoke(&mut self, id: u64) -> Option<String> {
        let i = self.list.iter().position(|p| p.id == id)?;
        let p = self.list.remove(i);
        self.save();
        Some(p.token)
    }

    pub fn revoke_all(&mut self) -> Vec<String> {
        let tokens = self.list.drain(..).map(|p| p.token).collect();
        self.save();
        tokens
    }

    pub fn role_of(&self, token: &str) -> Option<Role> {
        self.list
            .iter()
            .find(|p| crate::http::token_eq(&p.token, token))
            .map(|p| p.role)
    }

    pub fn name_of(&self, token: &str) -> Option<String> {
        self.list
            .iter()
            .find(|p| crate::http::token_eq(&p.token, token))
            .map(|p| p.name.clone())
    }

    /// The list for the state document (no tokens).
    pub fn public(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.list
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "id": p.id, "role": p.role, "name": p.name, "created_ms": p.created_ms
                    })
                })
                .collect(),
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn create_persist_revoke() {
        let dir = std::env::temp_dir().join(format!("sm-pair-{}", std::process::id()));
        let file = dir.join("pairings.json");
        let mut p = Pairings::load(Some(file.clone()));
        let a = p.create(Role::Control, "Stage iPad");
        let b = p.create(Role::Admin, " ");
        assert_eq!(a.token.len(), 32);
        assert_ne!(a.token, b.token);
        assert_eq!(b.role, Role::Control, "pairing never grants admin");
        assert_eq!(b.name, "Device 2");
        let again = Pairings::load(Some(file.clone()));
        assert_eq!(again.role_of(&a.token), Some(Role::Control));
        assert_eq!(again.role_of("nope"), None);
        assert!(!again.public().to_string().contains(&a.token));
        let mut again = again;
        assert_eq!(again.revoke(a.id), Some(a.token.clone()));
        assert_eq!(Pairings::load(Some(file.clone())).role_of(&a.token), None);
        assert_eq!(again.revoke_all().len(), 1);
        assert!(Pairings::load(Some(file))
            .public()
            .as_array()
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
