//! In-app updater.
//!
//! This is the only part of Show Master that ever talks to the internet. It
//! asks the public downloads repository for the newest release, and - only
//! when the operator presses the update button - downloads it, verifies its
//! SHA-256 checksum and installs it. Everything else works without a
//! network; when there is none the check simply reports "offline".
//!
//! Two kinds of download exist per release, as in Beamweave: a small
//! program-only file (`showmaster-<system>.gz`) used when the installed copy
//! can replace its own executable, and the full installer (`.dmg`, setup
//! `.exe`) otherwise.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const DOWNLOAD_PAGE: &str =
    "https://github.com/Cheter0410/show-master-releases/releases/latest";
const RELEASES: &str =
    "https://api.github.com/repos/Cheter0410/show-master-releases/releases/latest";
const TRUSTED_PREFIX: &str = "https://github.com/Cheter0410/show-master-releases/";
const AGENT: &str = "showmaster-updater";
const WORK_DIR: &str = "showmaster-update";
#[cfg(target_os = "macos")]
const APP_BUNDLE: &str = "ShowMaster.app";

#[derive(Clone, PartialEq, Debug)]
pub struct Asset {
    pub url: String,
    pub name: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub program: Option<Asset>,
    pub installer: Option<Asset>,
}

impl Release {
    /// The download to use and whether it is the program-only patch.
    pub fn chosen(&self) -> Option<(&Asset, bool)> {
        match (&self.program, &self.installer) {
            (Some(p), _) if can_patch() => Some((p, true)),
            (_, Some(i)) if can_install() => Some((i, false)),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum State {
    Checking,
    Current,
    Offline(String),
    Available(Release),
    Downloading(Release, u64, u64),
    Installing(Release),
    /// The new version is in place; the application must exit now.
    Quit,
    Failed(Release, String),
}

fn parse_version(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches('v')
        .split('.')
        .map(|p| {
            p.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (a, b) = (parse_version(candidate), parse_version(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

pub fn program_asset(system: &str) -> String {
    format!("showmaster-{system}.gz")
}

/// (program-only asset name, installer suffix) for this build, if it can update itself.
pub fn platform() -> Option<(String, &'static str)> {
    let (system, installer) = if cfg!(all(windows, target_arch = "x86_64")) {
        ("windows-x86_64", ".exe")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        ("macos-arm64", ".dmg")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        ("macos-x86_64", ".dmg")
    } else {
        return None;
    };
    Some((program_asset(system), installer))
}

fn asset(a: &Value) -> Option<Asset> {
    Some(Asset {
        url: a.get("browser_download_url")?.as_str()?.to_string(),
        name: a.get("name")?.as_str()?.to_string(),
        size: a.get("size").and_then(Value::as_u64).unwrap_or(0),
        sha256: a
            .get("digest")
            .and_then(Value::as_str)
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_lowercase),
    })
}

pub fn pick(json: &Value, program_name: &str, installer_suffix: &str) -> Option<Release> {
    let version = json
        .get("tag_name")?
        .as_str()?
        .trim_start_matches('v')
        .to_string();
    let assets: Vec<Asset> = json
        .get("assets")?
        .as_array()?
        .iter()
        .filter_map(asset)
        .collect();
    Some(Release {
        version,
        notes: json
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        program: assets.iter().find(|a| a.name == program_name).cloned(),
        installer: assets
            .iter()
            .find(|a| a.name.ends_with(installer_suffix))
            .cloned(),
    })
}

fn fetch_latest() -> Result<Value, String> {
    let resp = ureq::get(RELEASES)
        .set("User-Agent", AGENT)
        .set("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(15))
        .call();
    let text = match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        // No release published yet.
        Err(ureq::Error::Status(404, _)) => return Ok(Value::Null),
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("The update server answered with error {code}."))
        }
        Err(_) => return Err("Could not reach the update server (no internet connection).".into()),
    };
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

pub fn check(current: &str) -> Result<Option<Release>, String> {
    let json = fetch_latest()?;
    let (program, suffix) = platform().unwrap_or((String::new(), ".none"));
    Ok(pick(&json, &program, suffix).filter(|r| is_newer(&r.version, current)))
}

pub fn download(a: &Asset, progress: &dyn Fn(u64, u64)) -> Result<PathBuf, String> {
    if !a.url.starts_with(TRUSTED_PREFIX) {
        return Err("Unexpected download address.".into());
    }
    let dir = std::env::temp_dir().join(WORK_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe: String = a
        .name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    let path = dir.join(if safe.is_empty() {
        "update.bin".to_string()
    } else {
        safe
    });
    let resp = ureq::get(&a.url)
        .set("User-Agent", AGENT)
        .call()
        .map_err(|_| {
            "The download could not be started. Check your internet connection.".to_string()
        })?;
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|_| "The download was interrupted.".to_string())?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).ok_or("read past buffer")?;
        file.write_all(chunk).map_err(|e| e.to_string())?;
        hasher.update(chunk);
        done += n as u64;
        progress(done, a.size.max(done));
    }
    file.flush().map_err(|e| e.to_string())?;
    if a.size > 0 && done != a.size {
        return Err("The download was incomplete. Please try again.".into());
    }
    if let Some(want) = &a.sha256 {
        let got: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if &got != want {
            let _ = std::fs::remove_file(&path);
            return Err(
                "The downloaded file is damaged (checksum mismatch). Please try again.".into(),
            );
        }
    }
    Ok(path)
}

fn looks_like_program(head: &[u8]) -> bool {
    if cfg!(windows) {
        head.starts_with(b"MZ")
    } else if cfg!(target_os = "macos") {
        head.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]) || head.starts_with(&[0xca, 0xfe, 0xba, 0xbe])
    } else {
        head.starts_with(b"\x7fELF")
    }
}

/// Unpack a program-only download, refusing anything that is not an executable.
pub fn unpack(gz: &Path, dest: &Path) -> Result<(), String> {
    let mut dec = flate2::read::GzDecoder::new(std::fs::File::open(gz).map_err(|e| e.to_string())?);
    let mut out = std::fs::File::create(dest)
        .map_err(|_| "No permission to write into the program folder.".to_string())?;
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    let mut checked = false;
    loop {
        let n = dec
            .read(&mut buf)
            .map_err(|_| "The update file could not be unpacked.".to_string())?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).ok_or("read past buffer")?;
        if !checked {
            checked = true;
            if !looks_like_program(chunk) {
                drop(out);
                let _ = std::fs::remove_file(dest);
                return Err("The update file is not a valid program.".into());
            }
        }
        out.write_all(chunk).map_err(|e| e.to_string())?;
        total += n as u64;
    }
    out.flush().map_err(|e| e.to_string())?;
    if total < 1_000_000 {
        let _ = std::fs::remove_file(dest);
        return Err("The update file is too small to be valid.".into());
    }
    Ok(())
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn writable(dir: &Path) -> bool {
    let probe = dir.join(".showmaster-write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// The installed `.app`, if we run from one that can be replaced in place.
#[cfg(target_os = "macos")]
fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.ancestors().nth(3)?.to_path_buf();
    let s = app.to_string_lossy().into_owned();
    (s.ends_with(".app") && !s.contains("AppTranslocation") && !s.starts_with("/Volumes/"))
        .then_some(app)
}

/// Can this copy replace its own executable (program-only update)?
#[cfg(target_os = "macos")]
pub fn can_patch() -> bool {
    bundle().is_some() && exe_dir_writable()
}

/// Can this copy replace its own executable (program-only update)?
#[cfg(windows)]
pub fn can_patch() -> bool {
    exe_dir_writable()
}

/// Can this copy replace its own executable (program-only update)?
#[cfg(not(any(windows, target_os = "macos")))]
pub fn can_patch() -> bool {
    false
}

#[cfg(any(windows, target_os = "macos"))]
fn exe_dir_writable() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(writable))
        .unwrap_or(false)
}

/// Can this copy run the full installer by itself?
#[cfg(target_os = "macos")]
pub fn can_install() -> bool {
    bundle().is_some()
}

/// Can this copy run the full installer by itself?
#[cfg(not(target_os = "macos"))]
pub fn can_install() -> bool {
    cfg!(windows)
}

/// Remove leftovers of a finished update. Call once at start-up.
pub fn cleanup_after_update() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_file_name("showmaster.old.exe"));
        let _ = std::fs::remove_file(exe.with_file_name("showmaster.new"));
    }
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(WORK_DIR));
}

/// Wait for this process to exit, run `extra`, then start the app again.
#[cfg(target_os = "macos")]
fn relaunch_script(app: &Path, extra: &str) -> Result<(), String> {
    let script =
        format!("while kill -0 \"$2\" 2>/dev/null; do sleep 0.2; done\n{extra}\nopen \"$1\"\n");
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .arg(app)
        .arg(std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn install_program(gz: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let app =
        bundle().ok_or("Move Show Master into the Applications folder first, then update.")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let fresh = exe.with_file_name("showmaster.new");
    unpack(gz, &fresh)?;
    std::fs::set_permissions(&fresh, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    std::fs::rename(&fresh, &exe)
        .map_err(|_| "No permission to replace the program.".to_string())?;
    // The bundle is ad-hoc signed; re-sign after replacing the executable.
    relaunch_script(
        &app,
        "/usr/bin/codesign --force --deep -s - \"$1\" >/dev/null 2>&1",
    )
}

#[cfg(target_os = "macos")]
fn install_installer(dmg: &Path) -> Result<(), String> {
    let app =
        bundle().ok_or("Move Show Master into the Applications folder first, then update.")?;
    if !app.parent().map(writable).unwrap_or(false) {
        return Err(
            "No permission to replace the app. Please download the new version manually.".into(),
        );
    }
    let extra = format!(
        r#"MNT=$(mktemp -d /tmp/showmaster-mnt.XXXXXX) || exit 1
if hdiutil attach -nobrowse -readonly -mountpoint "$MNT" "{}" >/dev/null 2>&1 && [ -d "$MNT/{APP_BUNDLE}" ]; then
  rm -rf "$1.old"
  if mv "$1" "$1.old"; then
    if ditto "$MNT/{APP_BUNDLE}" "$1"; then rm -rf "$1.old"; else rm -rf "$1"; mv "$1.old" "$1"; fi
  fi
fi
hdiutil detach "$MNT" -quiet >/dev/null 2>&1
rmdir "$MNT" 2>/dev/null"#,
        dmg.to_string_lossy().replace('"', "")
    );
    relaunch_script(&app, &extra)
}

#[cfg(windows)]
fn install_program(gz: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let fresh = exe.with_file_name("showmaster.new");
    let old = exe.with_file_name("showmaster.old.exe");
    unpack(gz, &fresh)?;
    let _ = std::fs::remove_file(&old);
    // A running program cannot be overwritten on Windows, but it can be renamed.
    std::fs::rename(&exe, &old).map_err(|_| "No permission to replace the program.".to_string())?;
    if std::fs::rename(&fresh, &exe).is_err() {
        let _ = std::fs::rename(&old, &exe);
        return Err("The new program could not be put in place.".into());
    }
    std::process::Command::new(&exe)
        .arg("--open")
        .arg("--after-update")
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(windows)]
fn install_installer(file: &Path) -> Result<(), String> {
    std::process::Command::new(file)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn install_program(_: &Path) -> Result<(), String> {
    Err("Automatic install is not available on this system.".into())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn install_installer(_: &Path) -> Result<(), String> {
    Err("Automatic install is not available on this system.".into())
}

fn set(state: &Mutex<State>, value: State) {
    if let Ok(mut s) = state.lock() {
        *s = value;
    }
}

pub struct Updater {
    state: Arc<Mutex<State>>,
    current: String,
}

impl Updater {
    /// Create the updater and start the first check in the background.
    pub fn new(current_version: &str) -> Self {
        let u = Updater {
            state: Arc::new(Mutex::new(State::Checking)),
            current: current_version.to_string(),
        };
        u.recheck();
        u
    }

    pub fn recheck(&self) {
        let s = self.state.clone();
        let current = self.current.clone();
        set(&s, State::Checking);
        let spawned = std::thread::Builder::new()
            .name("sm-update".into())
            .spawn(move || {
                let next = match check(&current) {
                    Ok(Some(r)) => State::Available(r),
                    Ok(None) => State::Current,
                    Err(e) => State::Offline(e),
                };
                set(&s, next);
            });
        if spawned.is_err() {
            set(
                &self.state,
                State::Offline("could not start the update check".into()),
            );
        }
    }

    pub fn get(&self) -> State {
        self.state
            .lock()
            .map(|s| s.clone())
            .unwrap_or(State::Current)
    }

    pub fn busy(&self) -> bool {
        matches!(self.get(), State::Downloading(..) | State::Installing(_))
    }

    /// Download and install the available release. On success the state
    /// becomes [`State::Quit`] and the application must exit.
    pub fn start(&self) -> Result<(), String> {
        let r = match self.get() {
            State::Available(r) | State::Failed(r, _) => r,
            State::Downloading(..) | State::Installing(_) => {
                return Err("an update is already being installed".into())
            }
            _ => return Err("no update available".into()),
        };
        let s = self.state.clone();
        let Some((asset, patch)) = r.chosen().map(|(a, p)| (a.clone(), p)) else {
            set(
                &s,
                State::Failed(
                    r,
                    "This release has no automatic install for your system. Use the download page."
                        .into(),
                ),
            );
            return Ok(());
        };
        set(&s, State::Downloading(r.clone(), 0, asset.size));
        std::thread::Builder::new()
            .name("sm-update".into())
            .spawn(move || {
                let (s2, r2) = (s.clone(), r.clone());
                let result = download(&asset, &move |done, total| {
                    set(&s2, State::Downloading(r2.clone(), done, total))
                })
                .and_then(|file| {
                    set(&s, State::Installing(r.clone()));
                    if patch {
                        install_program(&file)
                    } else {
                        install_installer(&file)
                    }
                });
                set(
                    &s,
                    match result {
                        Ok(()) => State::Quit,
                        Err(e) => State::Failed(r, e),
                    },
                );
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// State for the UI.
    pub fn json(&self) -> Value {
        let base = |state: &str, r: Option<&Release>| {
            json!({
                "state": state,
                "current": self.current,
                "version": r.map(|r| r.version.clone()),
                "notes": r.map(|r| r.notes.clone()),
                "automatic": r.map(|r| r.chosen().is_some()),
                "download_page": DOWNLOAD_PAGE,
                "done": 0, "total": 0, "error": null,
            })
        };
        match self.get() {
            State::Checking => base("checking", None),
            State::Current => base("current", None),
            State::Offline(e) => {
                let mut v = base("offline", None);
                v["error"] = Value::from(e);
                v
            }
            State::Available(r) => base("available", Some(&r)),
            State::Downloading(r, done, total) => {
                let mut v = base("downloading", Some(&r));
                v["done"] = Value::from(done);
                v["total"] = Value::from(total);
                v
            }
            State::Installing(r) => base("installing", Some(&r)),
            State::Quit => base("restarting", None),
            State::Failed(r, e) => {
                let mut v = base("failed", Some(&r));
                v["error"] = Value::from(e);
                v
            }
        }
    }
}

/// What the UI sees when update checks are switched off.
pub fn disabled_json(current: &str) -> Value {
    json!({ "state": "disabled", "current": current, "version": null, "notes": null, "automatic": null,
            "download_page": DOWNLOAD_PAGE, "done": 0, "total": 0, "error": null })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_order() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v1.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
    }

    #[test]
    fn picks_program_and_installer() {
        let base = "https://github.com/Cheter0410/show-master-releases/releases/download/v0.3.1/";
        let json: Value = serde_json::from_str(&format!(
            r#"{{"tag_name":"v0.3.1","body":"- faster\n","assets":[
            {{"name":"ShowMaster-0.3.1.dmg","browser_download_url":"{base}ShowMaster-0.3.1.dmg","size":10,"digest":"sha256:ABCD"}},
            {{"name":"showmaster-macos-arm64.gz","browser_download_url":"{base}showmaster-macos-arm64.gz","size":5}},
            {{"name":"ShowMaster-Setup-0.3.1.exe","browser_download_url":"{base}ShowMaster-Setup-0.3.1.exe","size":20}}]}}"#
        ))
        .unwrap();
        let mac = pick(&json, &program_asset("macos-arm64"), ".dmg").unwrap();
        assert_eq!(
            (mac.version.as_str(), mac.notes.as_str()),
            ("0.3.1", "- faster")
        );
        assert_eq!(mac.program.as_ref().unwrap().size, 5);
        assert_eq!(
            mac.installer.as_ref().unwrap().sha256.as_deref(),
            Some("abcd")
        );
        let win = pick(&json, &program_asset("windows-x86_64"), ".exe").unwrap();
        assert!(win.program.is_none() && win.installer.unwrap().name.ends_with(".exe"));
        assert!(pick(&Value::Null, "x", ".y").is_none());
    }

    #[test]
    fn downloads_only_from_the_releases_repository() {
        let a = Asset {
            url: "https://example.com/showmaster.gz".into(),
            name: "x.gz".into(),
            size: 1,
            sha256: None,
        };
        assert_eq!(
            download(&a, &|_, _| {}).unwrap_err(),
            "Unexpected download address."
        );
    }

    #[test]
    fn unpack_accepts_programs_and_rejects_other_files() {
        use flate2::{write::GzEncoder, Compression};
        let dir = std::env::temp_dir().join(format!("showmaster-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let pack = |name: &str, data: &[u8]| {
            let p = dir.join(name);
            let mut e = GzEncoder::new(std::fs::File::create(&p).unwrap(), Compression::fast());
            e.write_all(data).unwrap();
            e.finish().unwrap();
            p
        };
        let good = pack("good.gz", &exe);
        let out = dir.join("out.bin");
        unpack(&good, &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), exe);
        let bad = pack("bad.gz", &vec![b'x'; 2_000_000]);
        assert!(unpack(&bad, &out).is_err());
        assert!(!out.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
