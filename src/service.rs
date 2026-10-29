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

/// Whether this is an OSTree/bootc style machine, where packages are layered
/// with `rpm-ostree` and a reboot instead of being installed, and where
/// Homebrew is the usual way to get a user-space tool.
pub fn atomic_host() -> bool {
    Path::new("/run/ostree-booted").exists()
}

/// The directories a user-space program can live in on a machine where nothing
/// may be installed system-wide: Homebrew (which Bluefin, Bazzite and the other
/// Silverblue derivatives ship), `~/.local/bin`, and the usual system ones.
pub fn program_dirs() -> Vec<PathBuf> {
    vec![
        home().join(".local/bin"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ]
}

/// The PATH the unit hands the daemon. A user service does not get the shell's
/// PATH, so without this a `qpdf` installed with Homebrew is invisible to it —
/// and the daemon refuses to run without `qpdf`.
pub fn unit_path() -> String {
    program_dirs()
        .iter()
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>()
        .join(":")
}

/// Look for a program the way the unit will find it: the session's PATH first,
/// then the user-space directories, so a Homebrew `qpdf` counts even when the
/// window was started from a session that never sourced `brew shellenv`.
pub fn find_program(name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        if let Some(found) = std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
        {
            return Some(found);
        }
    }
    program_dirs()
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// What a unit has to start: the daemon binary, or a copy of the AppImage plus
/// the `daemon` subcommand it dispatches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exec {
    pub path: PathBuf,
    pub arguments: Vec<String>,
}

/// Where the program keeps the files it writes for the user: the AppImage's
/// installed copy, and the device URI/PPD saved before a queue is switched.
pub fn data_dir() -> PathBuf {
    home().join(".local/share").join(UNIT)
}

/// Where an AppImage puts itself when it is installed as a service.
pub fn installed_image() -> PathBuf {
    data_dir().join(format!("{UNIT}.AppImage"))
}

/// The AppImage this process runs from, if any: the runtime exports its path in
/// `$APPIMAGE`. The mount behind it disappears when the image exits, so a unit
/// must never point at it.
pub fn running_image() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

/// Resolve what the unit should start. From an AppImage the image is copied to
/// `~/.local/share/ipp-duplexd-fork/` first, because a unit pointing into the
/// temporary mount starts a program that is gone the moment the window closes.
pub fn exec_for_unit() -> Result<Exec, String> {
    if let Some(image) = running_image() {
        let target = installed_image();
        if image != target {
            if let Some(dir) = target.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::copy(&image, &target).map_err(|e| format!("{}: {e}", target.display()))?;
            set_executable(&target)?;
        }
        return Ok(Exec {
            path: target,
            arguments: vec!["daemon".to_string()],
        });
    }
    let daemon = daemon_binary().ok_or_else(|| {
        format!("{UNIT} was not found next to the window, so there is nothing to start")
    })?;
    Ok(Exec {
        path: daemon,
        arguments: Vec::new(),
    })
}

fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).map_err(|e| format!("{}: {e}", path.display()))
}

/// The unit file's text. With the window it is bound to the graphical session:
/// it starts once the display variables exist and stops when they go away.
/// Without it there is nothing session-bound about the daemon, so it runs for
/// the whole login (it still answers `GET /flip` over ssh, with no desktop up).
pub fn unit_text(daemon: &Path, gui: bool) -> String {
    unit_text_with(
        &Exec {
            path: daemon.to_path_buf(),
            arguments: Vec::new(),
        },
        gui,
    )
}

/// [`unit_text`] for a command that needs arguments of its own, such as an
/// AppImage, whose first argument is the subcommand to run.
pub fn unit_text_with(exec: &Exec, gui: bool) -> String {
    let (session, wanted_by) = if gui {
        (
            "After=graphical-session.target\nPartOf=graphical-session.target\n",
            "graphical-session.target",
        )
    } else {
        ("", "default.target")
    };
    let arguments = if exec.arguments.is_empty() {
        String::new()
    } else {
        format!(" {}", exec.arguments.join(" "))
    };
    // The `-` in EnvironmentFile= makes the file optional: the printers live in
    // config.toml, so a machine that never wrote the .conf still starts.
    format!(
        "[Unit]\n\
         Description=Manual duplex virtual IPP printer\n\
         After=network.target\n\
         {session}\n\
         [Service]\n\
         Environment=PATH={}\n\
         EnvironmentFile=-%h/.config/ipp-duplexd-fork.conf\n\
         ExecStart={}{arguments} $IPP_DUPLEXD_FORK_ARGS\n\
         Restart=on-failure\n\
         RestartSec=5\n\n\
         [Install]\n\
         WantedBy={wanted_by}\n",
        unit_path(),
        exec.path.display()
    )
}

/// `systemctl --user status`, for the window's diagnostics.
pub fn status_text() -> String {
    tool_text(
        &systemctl(),
        &["--user", "status", UNIT, "--no-pager", "-l", "--lines=0"],
    )
}

/// The unit's recent journal, for the same dialog.
pub fn journal_text(lines: usize) -> String {
    let count = format!("-n{lines}");
    tool_text("journalctl", &["--user", "-u", UNIT, "--no-pager", &count])
}

/// Run a program and return what it said, stdout and stderr together: a
/// diagnostic is not the place to be picky about which stream a tool used.
fn tool_text(program: &str, args: &[&str]) -> String {
    match Command::new(program).args(args).output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            let errors = String::from_utf8_lossy(&out.stderr);
            if !errors.trim().is_empty() {
                text.push_str(errors.trim_end());
                text.push('\n');
            }
            text
        }
        Err(e) => format!("{program}: {e}\n"),
    }
}

/// Write the unit if the package did not, make sure the environment file the
/// unit names exists, and have systemd start the service now and at every
/// login. Idempotent: everything it writes is compared with what is there
/// first, so a second call from the window changes nothing.
/// The window's image and the copy the unit runs, when they are not the same
/// file: the desktop shortcut updates one, and the unit keeps running the other
/// until someone copies it over. Returns `(running, installed)`.
pub fn stale_image() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let running = running_image()?;
    let installed = installed_image();
    let size = |path: &std::path::Path| std::fs::metadata(path).ok().map(|m| m.len());
    match (size(&running), size(&installed)) {
        (Some(a), Some(b)) if a != b => Some((running, installed)),
        _ => None,
    }
}

/// Copy the running image over the installed one, so the unit starts the
/// version the user just installed.
pub fn refresh_image() -> Result<(), String> {
    let (running, installed) = stale_image().ok_or("nothing to update")?;
    if let Some(dir) = installed.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // The unit is running the file we are replacing: writing into it directly
    // fails with "Text file busy". Copy beside it and rename over it, which
    // swaps the directory entry and leaves the running process on its own
    // inode.
    let temporary = installed.with_extension("AppImage.new");
    std::fs::copy(&running, &temporary).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    std::fs::rename(&temporary, &installed).map_err(|e| e.to_string())?;
    return Ok(());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

pub fn install(gui: bool) -> Result<Install, String> {
    let user_unit = user_unit_path();
    let mut wrote_unit = false;
    // A packaged unit already points at the packaged daemon; rewriting it in
    // ~/.config would shadow the package's copy and go stale on upgrade.
    let unit = match packaged_unit() {
        Some(path) => path,
        None => {
            let exec = exec_for_unit()?;
            let text = unit_text_with(&exec, gui);
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
    // An AppImage installed itself as a stable copy, and that copy exists only
    // for the unit. Unlinking the image we are running from is harmless: the
    // mount is already open.
    let _ = std::fs::remove_file(installed_image());
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
    fn the_unit_gives_the_daemon_a_user_space_path() {
        let unit = unit_text(Path::new("/usr/bin/ipp-duplexd-fork"), true);
        assert!(unit.contains("Environment=PATH="), "{unit}");
        // Homebrew, because an atomic machine cannot install qpdf system-wide.
        assert!(unit.contains("/home/linuxbrew/.linuxbrew/bin"), "{unit}");
        assert!(unit.contains(".local/bin"), "{unit}");
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

    #[test]
    fn an_appimage_unit_runs_the_daemon_subcommand() {
        let exec = Exec {
            path: PathBuf::from("/home/x/.local/share/ipp-duplexd-fork/ipp-duplexd-fork.AppImage"),
            arguments: vec!["daemon".to_string()],
        };
        let unit = unit_text_with(&exec, true);
        assert!(
            unit.contains(
                "ExecStart=/home/x/.local/share/ipp-duplexd-fork/ipp-duplexd-fork.AppImage daemon \
                 $IPP_DUPLEXD_FORK_ARGS"
            ),
            "{unit}"
        );
    }
}
