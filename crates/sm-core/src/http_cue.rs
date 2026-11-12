//! HTTP cues: one plain HTTP/1.1 request to a device on the show network
//! (projectors, smart plugs, media servers, other software with a web API).
//!
//! Only `http://` - no TLS: show networks are local, devices rarely have
//! valid certificates, and Show Master carries no certificate store. The
//! request is built here; the I/O layer sends it over a raw TCP connection
//! and only logs the status line of the answer.

use serde::{Deserialize, Serialize};

pub const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct HttpRequest {
    /// GET, POST, PUT, PATCH or DELETE.
    pub method: String,
    /// `http://host[:port]/path?query`
    pub url: String,
    /// Request body (not sent with GET / DELETE when empty).
    pub body: String,
    /// `Content-Type` of the body.
    pub content_type: String,
}

impl Default for HttpRequest {
    fn default() -> Self {
        Self {
            method: "GET".into(),
            url: "http://192.168.1.50/".into(),
            body: String::new(),
            content_type: "application/json".into(),
        }
    }
}

/// `url` split into what a raw connection needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    /// Path and query, starting with `/`.
    pub path: String,
}

impl Target {
    /// `host:port` for connecting (IPv6 literals in brackets).
    pub fn addr(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

pub fn parse_url(url: &str) -> Result<Target, String> {
    let url = url.trim();
    if url.len() > 2048 {
        return Err("URL too long".into());
    }
    if url.starts_with("https://") {
        return Err(
            "HTTPS is not supported (no certificates in a show network) - use http://".into(),
        );
    }
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("\"{url}\" must start with http://"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => match rest.find('?') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        },
    };
    let path = if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_string()
    };
    if authority.contains('@') {
        return Err("user:password@ in the URL is not supported".into());
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6.split_once(']').ok_or("bad IPv6 address")?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse::<u16>().map_err(|_| format!("bad port \"{p}\""))?,
            None if after.is_empty() => 80,
            None => return Err("bad address".into()),
        };
        (h.to_string(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse::<u16>().map_err(|_| format!("bad port \"{p}\""))?,
            ),
            None => (authority.to_string(), 80),
        }
    };
    if host.is_empty() || port == 0 {
        return Err("no host in the URL".into());
    }
    if path.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("spaces are not allowed in the URL (use %20)".into());
    }
    Ok(Target { host, port, path })
}

impl HttpRequest {
    pub fn validate(&self) -> Result<Target, String> {
        if !METHODS.contains(&self.method.as_str()) {
            return Err(format!("unknown HTTP method \"{}\"", self.method));
        }
        if self.content_type.chars().any(|c| c.is_control()) {
            return Err("bad content type".into());
        }
        parse_url(&self.url)
    }

    /// The complete request (connection closed after the answer).
    pub fn encode(&self) -> Result<(Target, Vec<u8>), String> {
        let t = self.validate()?;
        let host = if t.port == 80 {
            t.host.clone()
        } else {
            format!("{}:{}", t.host, t.port)
        };
        let mut head = format!(
            "{} {} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: ShowMaster\r\nConnection: close\r\n",
            self.method, t.path
        );
        let send_body =
            !self.body.is_empty() || matches!(self.method.as_str(), "POST" | "PUT" | "PATCH");
        if send_body {
            if !self.content_type.is_empty() {
                head.push_str(&format!("Content-Type: {}\r\n", self.content_type));
            }
            head.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        }
        head.push_str("\r\n");
        let mut bytes = head.into_bytes();
        if send_body {
            bytes.extend_from_slice(self.body.as_bytes());
        }
        Ok((t, bytes))
    }

    /// `POST http://10.0.0.5/api/scene`
    pub fn summary(&self) -> String {
        format!("{} {}", self.method, self.url)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let t = parse_url("http://10.0.0.5:8080/api/x?y=1").unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.path.as_str()),
            ("10.0.0.5", 8080, "/api/x?y=1")
        );
        assert_eq!(parse_url("http://beamer.local").unwrap().path, "/");
        assert_eq!(parse_url("http://h?x=1").unwrap().path, "/?x=1");
        let t = parse_url("http://[fe80::1]:81/a").unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.addr()),
            ("fe80::1", 81, "[fe80::1]:81".to_string())
        );
        assert!(parse_url("https://x/").unwrap_err().contains("HTTPS"));
        assert!(parse_url("ftp://x/").is_err());
        assert!(parse_url("http://:80/").is_err());
        assert!(parse_url("http://a:99999/").is_err());
        assert!(parse_url("http://u:p@a/").is_err());
        assert!(parse_url("http://a/b c").is_err());
    }

    #[test]
    fn requests() {
        let get = HttpRequest {
            url: "http://10.0.0.5/on".into(),
            ..HttpRequest::default()
        };
        let (_, b) = get.encode().unwrap();
        assert_eq!(
            String::from_utf8(b).unwrap(),
            "GET /on HTTP/1.1\r\nHost: 10.0.0.5\r\nUser-Agent: ShowMaster\r\nConnection: close\r\n\r\n"
        );
        let post = HttpRequest {
            method: "POST".into(),
            url: "http://h:8000/scene".into(),
            body: r#"{"scene":3}"#.into(),
            content_type: "application/json".into(),
        };
        let (t, b) = post.encode().unwrap();
        assert_eq!(t.addr(), "h:8000");
        let s = String::from_utf8(b).unwrap();
        assert!(s.starts_with("POST /scene HTTP/1.1\r\nHost: h:8000\r\n"));
        assert!(s.contains("Content-Length: 11\r\n"));
        assert!(s.ends_with("\r\n\r\n{\"scene\":3}"));
        assert!(HttpRequest {
            method: "BREW".into(),
            ..get
        }
        .encode()
        .is_err());
    }
}
