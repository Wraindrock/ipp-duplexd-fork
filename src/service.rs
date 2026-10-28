//! The systemd user service, as far as the front ends need it: whether one can
//! be controlled, how to restart it after the configuration changed, and how to
//! install it on a machine that has none yet (the window's first run).
//!
//! The daemon is a user service on machines that have systemd, and a plain
//! process everywhere else. This module never decides anything: it reports what
//! the tools said, and the window shows that. The one exception is
//! [`install`], which writes the unit and asks systemd to start it — the same
//! two steps the terminal wizard takes, so a machine can be set up from the
//! window alone.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Unit name; the package installs `ipp-duplexd-fork.service`.
pub const UNIT: &str = "ipp-duplexd-fork";

/// Overrides the `systemctl` program; tests use it to point at a stub.
pub const SYSTEMCTL_ENV: &str = "IPP_DUPLEXD_FORK_SYSTEMCTL";
/// Overrides where a package-installed unit is looked for; tests use it to say
/// that the machine has none.
pub const PACKAGED_ENV: &str = "IPP_DUPLEXD_FORK_PACKAGED_UNIT";
/// Overrides the daemon binary the written unit is pointed at; tests use a stub,
/// so that they do not depend on a build tree being present.
pub const DAEMON_ENV: &str = "IPP_DUPLEXD_FORK_DAEMON";

/// Where distributions put the unit: Fedora uses the first, Debian and Arch the
/// second, and systemd reads both.
pub const PACKAGED_UNITS: &[&str] = &[
    "/usr/lib/systemd/user/ipp-duplexd-fork.service",
    "/lib/systemd/user/ipp-duplexd-fork.service",
];

/// What a unit of our own starts with when the user never wrote a `.conf`:
/// comments only, so the daemon runs on `config.toml` alone.
const ENVIRONMENT_BOOTSTRAP: &str = "\
# Written by the ipp-duplexd-fork window.
#
# The virtual printers themselves live in
# ~/.config/ipp-duplexd-fork/config.toml. This file exists because the systemd
# unit names it and passes whatever IPP_DUPLEXD_FORK_ARGS it holds to the
# daemon, for example:
#   IPP_DUPLEXD_FORK_ARGS=--lang zh-CN
";

/// What [`install`] did, so the window can say something specific about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    /// The unit systemd will read: the package's, or the one written here.
    pub unit: PathBuf,
    /// Whether this run wrote the unit.
    pub wrote_unit: bool,
    /// Whether this run created `~/.config/ipp-duplexd-fork.conf`.
    pub wrote_environment: bool,
}

fn systemctl() -> String {
    std::env::var(SYSTEMCTL_ENV).unwrap_or_else(|_| "systemctl".to_string())
}

/// Run `systemctl` and return its output, or its own error message (so a
/// missing unit reads as "Unit … not found" instead of a phrase of ours).
fn run(args: &[&str]) -> Result<String, String> {
    let out = Command::new(systemctl())
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let message = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if message.is_empty() {
        format!("systemctl exited with {}", out.status)
    } else {
        message
    })
}

/// Whether `systemctl --user` answers at all (no systemd, no user manager).
pub fn available() -> bool {
    run(&["--user", "show-environment"]).is_ok()
}

/// Whether the unit reports itself as active (best effort).
pub fn is_active() -> bool {
    Command::new(systemctl())
        .args(["--user", "is-active", UNIT])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Restart the unit, starting it if it was not running. The error is the
/// tool's own message, so a missing unit reads as "Unit … not found".
pub fn restart() -> Result<(), String> {
    run(&["--user", "restart", UNIT]).map(|_| ())
}

/// The unit's own description of itself, for the window's service row.
pub fn state_text() -> String {
    run(&["--user", "is-active", UNIT]).unwrap_or_default()
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.config`, which is where systemd looks for user units (`%h` in the unit
/// means exactly this directory, so the two cannot drift apart).
fn config_dir() -> PathBuf {
    home().join(".config")
}

/// The unit a user install would write.
pub fn user_unit_path() -> PathBuf {
    config_dir()
        .join("systemd")
        .join("user")
        .join(format!("{UNIT}.service"))
}

/// The environment file the unit names. The unit spells it `%h/.config/...`, so
/// this is `$HOME/.config` even when `XDG_CONFIG_HOME` points elsewhere. The
/// package ships an example; the window creates a comment-only one when the
/// machine has neither.
pub fn environment_file() -> PathBuf {
    config_dir().join("ipp-duplexd-fork.conf")
}

/// The unit the package installed, if any.
pub fn packaged_unit() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(PACKAGED_ENV) {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    PACKAGED_UNITS
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

/// Whether systemd has a unit to start, packaged or written by us.
pub fn installed() -> bool {
    packaged_unit().is_some() || user_unit_path().is_file()
}

/// The daemon to point a unit at: next to the window first (a `cargo build`
/// tree, or `/usr/bin` for packages), then `PATH`.
pub fn daemon_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(DAEMON_ENV) {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    if let Ok(me) = std::env::current_exe() {
        let sibling = me.with_file_name("ipp-duplexd-fork");
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    std::env::var_os("PATH").and_then(|dirs| {
        std::env::split_paths(&dirs)
            .map(|dir| dir.join("ipp-duplexd-fork"))
            .find(|path| path.is_file())
    })
}

/// The unit file's text. With the window it is bound to the graphical session:
/// it starts once the display variables exist and stops when they go away.
/// Without it there is nothing session-bound about the daemon, so it runs for
/// the whole login (it still answers `GET /flip` over ssh, with no desktop up).
pub fn unit_text(daemon: &Path, gui: bool) -> String {
    let (session, wanted_by) = if gui {
        (
            "After=graphical-session.target\nPartOf=graphical-session.target\n",
            "graphical-session.target",
        )
    } else {
        ("", "default.target")
    };
    // The `-` in EnvironmentFile= makes the file optional: the printers live in
    // config.toml, so a machine that never wrote the .conf still starts.
    format!(
        "[Unit]\n\
         Description=Manual duplex virtual IPP printer\n\
         After=network.target\n\
         {session}\n\
         [Service]\n\
         EnvironmentFile=-%h/.config/ipp-duplexd-fork.conf\n\
         ExecStart={} $IPP_DUPLEXD_FORK_ARGS\n\
         Restart=on-failure\n\
         RestartSec=5\n\n\
         [Install]\n\
         WantedBy={wanted_by}\n",
        daemon.display()
    )
}

/// Write the unit if the package did not, make sure the environment file the
/// unit names exists, and have systemd start the service now and at every
/// login. Idempotent: everything it writes is compared with what is there
/// first, so a second call from the window changes nothing.
pub fn install(gui: bool) -> Result<Install, String> {
    let user_unit = user_unit_path();
    let mut wrote_unit = false;
    // A packaged unit already points at the packaged daemon; rewriting it in
    // ~/.config would shadow the package's copy and go stale on upgrade.
    let unit = match packaged_unit() {
        Some(path) => path,
        None => {
            let daemon = daemon_binary().ok_or_else(|| {
                format!("{UNIT} was not found next to the window, so there is nothing to start")
            })?;
            let text = unit_text(&daemon, gui);
            let same = std::fs::read_to_string(&user_unit)
                .map(|old| old == text)
                .unwrap_or(false);
            if !same {
                if let Some(dir) = user_unit.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                std::fs::write(&user_unit, &text)
                    .map_err(|e| format!("{}: {e}", user_unit.display()))?;
                wrote_unit = true;
            }
            user_unit
        }
    };
    let environment = environment_file();
    let mut wrote_environment = false;
    if !environment.is_file() {
        if let Some(dir) = environment.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&environment, ENVIRONMENT_BOOTSTRAP)
            .map_err(|e| format!("{}: {e}", environment.display()))?;
        wrote_environment = true;
    }
    run(&["--user", "daemon-reload"])?;
    run(&["--user", "enable", "--now", UNIT])
        .map_err(|e| format!("systemctl --user enable --now {UNIT}: {e}"))?;
    // A unit that was already running still holds the old unit file or the old
    // configuration; one restart makes both true again.
    let _ = run(&["--user", "restart", UNIT]);
    Ok(Install {
        unit,
        wrote_unit,
        wrote_environment,
    })
}

/// What [`uninstall`] did, so the window can say something specific about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uninstall {
    /// The unit that was in use: the package's, or the one written here.
    pub unit: PathBuf,
    /// Whether this run deleted the unit (only ever one we wrote).
    pub removed_unit: bool,
    /// Whether `systemctl --user disable --now` succeeded.
    pub stopped: bool,
    /// Whether the unit belongs to the package, which keeps it.
    pub packaged: bool,
}

/// The other half of [`install`]: stop and disable the service, and delete the
/// unit if this program wrote it. The configuration file, the printers and
/// their CUPS queues are left alone — they are the user's, and installing again
/// brings the service back behind them. Nothing here needs root: a unit in
/// `/usr` belongs to the package and is only stopped.
pub fn uninstall() -> Result<Uninstall, String> {
    let packaged = packaged_unit();
    let user_unit = user_unit_path();
    let unit = packaged.clone().unwrap_or_else(|| user_unit.clone());
    // Stop it first: while the daemon runs it holds the listen sockets and CUPS
    // keeps sending jobs to a service that is about to disappear.
    let stopped = run(&["--user", "disable", "--now", UNIT]).is_ok();
    let mut removed_unit = false;
    if packaged.is_none() && user_unit.is_file() {
        std::fs::remove_file(&user_unit).map_err(|e| format!("{}: {e}", user_unit.display()))?;
        removed_unit = true;
    }
    // Best effort: with the file gone the next reload forgets the unit anyway,
    // and a machine without systemd still had its unit removed.
    let _ = run(&["--user", "daemon-reload"]);
    // The .conf exists only because the unit names it. Drop it when it is still
    // the comment block this program wrote; anything else is the user's.
    let environment = environment_file();
    if let Ok(text) = std::fs::read_to_string(&environment) {
        if text == ENVIRONMENT_BOOTSTRAP {
            let _ = std::fs::remove_file(&environment);
        }
    }
    Ok(Uninstall {
        unit,
        removed_unit,
        stopped,
        packaged: packaged.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_starts_the_daemon_next_to_the_window() {
        let unit = unit_text(Path::new("/usr/bin/ipp-duplexd-fork"), true);
        assert!(
            unit.contains("ExecStart=/usr/bin/ipp-duplexd-fork $IPP_DUPLEXD_FORK_ARGS"),
            "{unit}"
        );
        assert!(
            unit.contains("EnvironmentFile=-%h/.config/ipp-duplexd-fork.conf"),
            "{unit}"
        );
        assert!(unit.contains("PartOf=graphical-session.target"), "{unit}");
        assert!(unit.contains("WantedBy=graphical-session.target"), "{unit}");
        assert!(unit.contains("Restart=on-failure"), "{unit}");
    }

    #[test]
    fn without_the_window_the_unit_runs_for_the_whole_login() {
        let unit = unit_text(Path::new("/usr/bin/ipp-duplexd-fork"), false);
        assert!(!unit.contains("graphical-session"), "{unit}");
        assert!(unit.contains("WantedBy=default.target"), "{unit}");
    }

    #[test]
    fn the_unit_is_named_after_the_service() {
        let path = user_unit_path();
        assert!(
            path.ends_with("systemd/user/ipp-duplexd-fork.service"),
            "{path:?}"
        );
    }
}
