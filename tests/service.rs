//! Installing the user service from the window: the unit it writes, the
//! environment file it fills in, and the systemd calls it makes. `systemctl` is
//! a stub script, so this runs on a machine with no systemd at all — and on a
//! machine where the real units must not be touched.

use ipp_duplexd_fork::service;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The overrides are process-wide, so the tests that set them take turns.
fn guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ipp-service-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A script that stands in for a program; returns its path.
fn stub(path: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let mut file = std::fs::File::create(path).unwrap();
    use std::io::Write;
    writeln!(file, "#!/bin/sh\n{body}").unwrap();
    drop(file);
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
    path.to_path_buf()
}

/// Everything `install` needs to be faked: a home, a daemon, a systemctl that
/// records what it was asked to do, and "no packaged unit".
struct Machine {
    dir: PathBuf,
    home: PathBuf,
    daemon: PathBuf,
    calls: PathBuf,
}

impl Machine {
    fn new(tag: &str, packaged: Option<&Path>) -> Self {
        let dir = tempdir(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let daemon = stub(&dir.join("ipp-duplexd-fork"), "exit 0");
        let calls = dir.join("calls");
        let systemctl = stub(
            &dir.join("systemctl"),
            &format!("echo \"$@\" >> {}\nexit 0", calls.display()),
        );
        std::env::set_var("HOME", &home);
        std::env::set_var(service::DAEMON_ENV, &daemon);
        std::env::set_var(
            service::PACKAGED_ENV,
            packaged.unwrap_or_else(|| Path::new("")),
        );
        std::env::set_var(service::SYSTEMCTL_ENV, &systemctl);
        Machine {
            dir,
            home,
            daemon,
            calls,
        }
    }

    fn unit(&self) -> PathBuf {
        self.home
            .join(".config/systemd/user/ipp-duplexd-fork.service")
    }

    fn environment(&self) -> PathBuf {
        self.home.join(".config/ipp-duplexd-fork.conf")
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(&self.calls).unwrap_or_default()
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        std::env::remove_var(service::DAEMON_ENV);
        std::env::remove_var(service::PACKAGED_ENV);
        std::env::remove_var(service::SYSTEMCTL_ENV);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn installing_writes_the_unit_the_environment_file_and_starts_it() {
    let _guard = guard();
    let machine = Machine::new("install", None);

    let done = service::install(true).expect("install into a fresh home");

    assert!(done.wrote_unit, "the unit should be new");
    assert!(done.wrote_environment, "the .conf should be new");
    assert_eq!(done.unit, machine.unit());
    let unit = std::fs::read_to_string(machine.unit()).unwrap();
    assert!(
        unit.contains(&format!(
            "ExecStart={} $IPP_DUPLEXD_FORK_ARGS",
            machine.daemon.display()
        )),
        "{unit}"
    );
    assert!(unit.contains("PartOf=graphical-session.target"), "{unit}");
    assert!(
        unit.contains("EnvironmentFile=-%h/.config/ipp-duplexd-fork.conf"),
        "{unit}"
    );
    assert!(machine.environment().is_file(), "the .conf the unit names");

    let calls = machine.calls();
    assert!(calls.contains("--user daemon-reload"), "{calls}");
    assert!(
        calls.contains("--user enable --now ipp-duplexd-fork"),
        "{calls}"
    );
}

#[test]
fn a_packaged_unit_is_left_alone_and_only_enabled() {
    let _guard = guard();
    let dir = tempdir("packaged");
    let packaged = dir.join("ipp-duplexd-fork.service");
    std::fs::write(&packaged, "[Unit]\nDescription=from the package\n").unwrap();
    let machine = Machine::new("packaged-home", Some(&packaged));

    let done = service::install(true).expect("install with a packaged unit");

    assert!(!done.wrote_unit, "a packaged unit must not be shadowed");
    assert_eq!(done.unit, packaged);
    assert!(!machine.unit().is_file(), "nothing should be written here");
    assert!(
        machine
            .calls()
            .contains("--user enable --now ipp-duplexd-fork"),
        "{}",
        machine.calls()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn installing_twice_changes_nothing() {
    let _guard = guard();
    let machine = Machine::new("twice", None);
    service::install(true).expect("first install");
    let first = std::fs::read_to_string(machine.unit()).unwrap();

    let done = service::install(true).expect("second install");

    assert!(!done.wrote_unit, "the second run should find the unit");
    assert!(
        !done.wrote_environment,
        "the second run should find the .conf"
    );
    assert_eq!(std::fs::read_to_string(machine.unit()).unwrap(), first);
}

#[test]
fn without_a_daemon_the_install_says_what_is_missing() {
    let _guard = guard();
    let machine = Machine::new("no-daemon", None);
    std::env::set_var(service::DAEMON_ENV, machine.dir.join("nowhere"));

    let error = service::install(true).unwrap_err();

    assert!(error.contains("ipp-duplexd-fork"), "{error}");
    assert!(!machine.calls().contains("enable"), "systemd was not asked");
}
