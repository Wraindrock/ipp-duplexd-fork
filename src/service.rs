//! The systemd user service, as far as the window needs it: whether it can
//! control one, and how to restart it after the configuration changed.
//!
//! The daemon is a user service on machines that have systemd, and a plain
//! process everywhere else. This module never decides anything: it reports
//! what the tools said, and the window shows that.

use std::process::Command;

/// Unit name; the package installs `ipp-duplexd-fork.service`.
pub const UNIT: &str = "ipp-duplexd-fork";

/// Whether `systemctl --user` answers at all (no systemd, no user manager).
pub fn available() -> bool {
    Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Whether the unit reports itself as active (best effort).
pub fn is_active() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-active", UNIT])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Restart the unit, starting it if it was not running. The error is the
/// tool's own message, so a missing unit reads as "Unit … not found".
pub fn restart() -> Result<(), String> {
    let out = Command::new("systemctl")
        .args(["--user", "restart", UNIT])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if message.is_empty() {
        format!("systemctl exited with {}", out.status)
    } else {
        message
    })
}

/// The unit's own description of itself, for the window's service row.
pub fn state_text() -> String {
    Command::new("systemctl")
        .args(["--user", "is-active", UNIT])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}
