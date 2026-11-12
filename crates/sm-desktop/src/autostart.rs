//! "Start Show Master with this computer" (per user, no admin rights):
//!
//! * Linux: `~/.config/autostart/showmaster.desktop` (XDG autostart),
//! * macOS: `~/Library/LaunchAgents/com.showmaster.app.plist` (RunAtLoad),
//! * Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value
//!   `ShowMaster` (set with `reg.exe`, no extra library).

use std::path::PathBuf;

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(target_os = "linux")]
fn entry() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))?;
    Some(base.join("autostart").join("showmaster.desktop"))
}

#[cfg(target_os = "macos")]
fn entry() -> Option<PathBuf> {
    Some(home()?.join("Library/LaunchAgents/com.showmaster.app.plist"))
}

#[cfg(target_os = "windows")]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// The program to start (this executable).
fn program() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string())
}

pub fn is_enabled() -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        entry().is_some_and(|p| p.exists())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("reg")
            .args(["query", RUN_KEY, "/v", "ShowMaster"])
            .output()
            .is_ok_and(|o| o.status.success())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
fn linux_set(path: &std::path::Path, on: bool) -> Result<(), String> {
    if !on {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    let exe = program()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=Show Master\nExec=\"{}\"\nIcon=showmaster\n\
         X-GNOME-Autostart-enabled=true\nComment=Started with the computer (Show Master > Setup > General)\n",
        exe.display()
    );
    std::fs::write(path, text).map_err(|e| e.to_string())
}

pub fn set_enabled(on: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let path = entry().ok_or("no home folder")?;
        linux_set(&path, on)
    }
    #[cfg(target_os = "macos")]
    {
        let path = entry().ok_or("no home folder")?;
        if !on {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            };
        }
        let exe = program()?;
        let esc = exe
            .display()
            .to_string()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\n\
             <key>Label</key><string>com.showmaster.app</string>\n\
             <key>ProgramArguments</key><array><string>{esc}</string></array>\n\
             <key>RunAtLoad</key><true/>\n\
             </dict></plist>\n"
        );
        std::fs::write(&path, text).map_err(|e| e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("reg");
        if on {
            let exe = program()?;
            cmd.args([
                "add",
                RUN_KEY,
                "/v",
                "ShowMaster",
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{}\"", exe.display()),
                "/f",
            ]);
        } else {
            if !is_enabled() {
                return Ok(());
            }
            cmd.args(["delete", RUN_KEY, "/v", "ShowMaster", "/f"]);
        }
        let out = cmd.output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = on;
        Err("not supported on this system".into())
    }
}

#[cfg(all(test, target_os = "linux"))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn linux_autostart_entry() {
        let dir = std::env::temp_dir().join(format!("sm-autostart-{}", std::process::id()));
        let path = dir.join("autostart/showmaster.desktop");
        linux_set(&path, true).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("Exec=\""));
        linux_set(&path, false).unwrap();
        assert!(!path.exists());
        linux_set(&path, false).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
