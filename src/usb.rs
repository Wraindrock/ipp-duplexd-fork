//! USB printers: `ipp-usb` is what turns one into an IPP printer.
//!
//! A printer on the end of a USB cable does not speak IPP. [`ipp-usb`][ipp-usb]
//! drives the device and publishes it on `ipp://localhost:60000/ipp/print`,
//! which is what a virtual printer's `real` then points at. Nothing in this
//! module touches USB: the question is only "does a configured `real` look like
//! that endpoint while the program is missing?", so it answers without hardware
//! and can be tested anywhere — which is the point, since the alternative is a
//! job that sits there until it times out with no explanation.
//!
//! [ipp-usb]: https://github.com/OpenPrinting/ipp-usb

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

/// The first port `ipp-usb` will allocate (`http-min-port` in its config).
pub const PORT: u16 = 60000;

/// The last port worth probing.
///
/// ipp-usb allocates from [`PORT`] upwards until it finds a free one, and its
/// stock configuration allows anything up to 65535. Scanning a thousand
/// loopback ports costs a few tens of milliseconds (a closed port refuses at
/// once), so the window can afford to look before deciding that nothing serves.
pub const PORT_END: u16 = 61000;

/// The program looked for in `PATH`.
pub const PROGRAM: &str = "ipp-usb";

/// Is `uri` the endpoint `ipp-usb` publishes: a loopback host on [`PORT`]?
///
/// Deliberately narrow. A local CUPS queue (`127.0.0.1:631/printers/…`) is the
/// recommended way to reach a printer and does not need `ipp-usb`, so warning
/// about it would be wrong.
pub fn is_endpoint(uri: &str) -> bool {
    match authority(uri) {
        Some((host, Some(port))) => port == PORT && is_loopback(&host),
        _ => false,
    }
}

/// The host and port of an `ipp://`, `ipps://` or `http://` URI.
///
/// Only what the check needs: user info is dropped, an IPv6 literal keeps its
/// brackets in the URI but not here, and a missing port stays `None`.
fn authority(uri: &str) -> Option<(String, Option<u16>)> {
    let (scheme, rest) = uri.split_once("://")?;
    // The daemon only ever talks IPP (and plain HTTP to it), so anything else
    // is not a printer it could forward to.
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "ipp" | "ipps" | "http" | "https"
    ) {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = after.strip_prefix(':').and_then(|p| p.parse().ok());
        return Some((host.to_ascii_lowercase(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_ascii_lowercase(), Some(port.parse().ok()?))),
        None => Some((authority.to_ascii_lowercase(), None)),
    }
}

/// `localhost`, `127.0.0.0/8` or `::1` — the only hosts `ipp-usb` listens on.
fn is_loopback(host: &str) -> bool {
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

/// Is `ipp-usb` installed? A `PATH` lookup rather than a process, so the status
/// page can ask on every request.
pub fn installed() -> bool {
    installed_in(std::env::var_os("PATH").as_deref())
}

/// The lookup half of [`installed`], with the search path passed in so tests do
/// not have to change the environment of the whole process.
pub fn installed_in(path: Option<&OsStr>) -> bool {
    let Some(path) = path else {
        return false;
    };
    std::env::split_paths(path).any(|dir| is_executable(&dir.join(PROGRAM)))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Is `ipp-usb` actually serving IPP on its port right now?
///
/// Being on `PATH` only says the package is installed. Whether it holds the
/// printer is a different question: when nothing answers, hplip owns the device
/// and printing through it is fine, so there is no conflict to report — and
/// pointing a queue at a dead port would only break it.
pub fn serving() -> bool {
    serving_port().is_some()
}

/// The port `ipp-usb` answers on, when it answers at all.
///
/// Its configuration allocates the port from `http-min-port` upwards, not
/// necessarily the first one: if 60000 was taken when a device instance
/// started, the printer is served on 60001, 60002, … and a check that only
/// looks at 60000 would report "not serving" while it plainly is.
pub fn serving_port() -> Option<u16> {
    (PORT..=PORT_END).find(|port| serving_at("127.0.0.1", *port))
}

/// The ipp-usb port a device URI points at, when it points at one of ours.
///
/// Accepts both spellings CUPS may keep (`localhost` and `127.0.0.1`) and any
/// port in the range ipp-usb allocates from.
pub fn ipp_usb_port_of(device_uri: &str) -> Option<u16> {
    let rest = device_uri
        .strip_prefix("ipp://localhost:")
        .or_else(|| device_uri.strip_prefix("ipp://127.0.0.1:"))?;
    let (port, path) = rest.split_once('/')?;
    if !path.starts_with("ipp/print") {
        return None;
    }
    let port: u16 = port.parse().ok()?;
    (PORT..=PORT_END).contains(&port).then_some(port)
}

/// [`serving`] for any host and port, so tests can stand up a listener.
pub fn serving_at(host: &str, port: u16) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let Ok(mut addresses) = (host, port).to_socket_addrs() else {
        return false;
    };
    addresses.any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok())
}

/// Overrides the `systemctl` program; tests use it to point at a stub.
pub const SYSTEMCTL_ENV: &str = "IPP_DUPLEXD_FORK_SYSTEMCTL";

/// Is the `ipp-usb` *service* running?
///
/// Different from [`serving`]: the service can be active (so it has claimed the
/// USB device and hplip cannot open it) while no device instance answers on the
/// port. That gap is the state where neither path can print.
pub fn service_active() -> bool {
    let program = std::env::var(SYSTEMCTL_ENV).unwrap_or_else(|_| "systemctl".to_string());
    std::process::Command::new(program)
        .args(["is-active", "ipp-usb"])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Overrides the USB sysfs root; tests use a temporary directory.
pub const SYSFS_ENV: &str = "IPP_DUPLEXD_FORK_SYSFS_USB";

/// Is a USB device with this serial plugged into this machine?
///
/// `None` when the answer is not knowable (no sysfs, no serial), so a caller
/// never claims "unplugged" on a guess. A printer that is passed through to a
/// virtual machine is gone from the host's sysfs, which is exactly the case
/// where a hplip queue stops working for no visible reason.
pub fn attached(serial: &str) -> Option<bool> {
    let root = std::env::var_os(SYSFS_ENV)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/sys/bus/usb/devices"));
    let entries = std::fs::read_dir(&root).ok()?;
    let mut seen = false;
    for entry in entries.flatten() {
        let path = entry.path().join("serial");
        let Ok(serial_of_device) = std::fs::read_to_string(&path) else {
            continue;
        };
        seen = true;
        if serial_of_device.trim() == serial {
            return Some(true);
        }
    }
    // No device in sysfs exposes a serial at all: we cannot tell whether ours
    // is missing or simply not reporting one.
    if seen {
        Some(false)
    } else {
        None
    }
}

/// The first printer that cannot print because `ipp-usb` is missing.
///
/// `uris` are the configured `real` addresses and `installed` is passed in (not
/// looked up) so the caller decides when to pay for the lookup and the tests
/// decide the answer.
pub fn missing<'a>(uris: impl IntoIterator<Item = &'a str>, installed: bool) -> Option<&'a str> {
    if installed {
        return None;
    }
    uris.into_iter().find(|uri| is_endpoint(uri))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_endpoint_is_loopback_60000() {
        for uri in [
            "ipp://localhost:60000/ipp/print",
            "ipp://127.0.0.1:60000/ipp/print",
            "http://LOCALHOST:60000/ipp/print",
            "ipp://[::1]:60000/ipp/print",
            "ipp://user:pw@127.0.0.1:60000/ipp/print",
            "ipp://127.0.0.1:60000",
            "ipp://127.0.0.1:60000/",
        ] {
            assert!(is_endpoint(uri), "{uri} should count as ipp-usb");
        }
    }

    #[test]
    fn other_printers_are_not_ipp_usb() {
        for uri in [
            // a local CUPS queue, the recommended way to reach a printer
            "ipp://127.0.0.1:631/printers/laser",
            "ipp://localhost:631/printers/laser",
            // USB device on another machine, or a different ipp-usb port
            "ipp://192.168.1.50:60000/ipp/print",
            "ipps://printer.local:60000/ipp/print",
            "ipp://localhost:60001/ipp/print",
            "ipp://localhost/ipp/print",
            // not a URI at all
            "",
            "localhost:60000",
            "ftp://127.0.0.1:60000/ipp/print",
        ] {
            assert!(!is_endpoint(uri), "{uri} is not an ipp-usb endpoint");
        }
    }

    #[test]
    fn missing_reports_only_uncovered_usb_printers() {
        let usb = "ipp://localhost:60000/ipp/print";
        let cups = "ipp://127.0.0.1:631/printers/laser";
        // installed: nothing to say
        assert_eq!(missing([usb], true), None);
        // missing and in use: say it once, with the URI to show
        assert_eq!(missing([usb], false), Some(usb));
        // missing but nothing needs it: silence
        assert_eq!(missing([cups], false), None);
        assert_eq!(missing([], false), None);
        // the first offender wins, order preserved
        assert_eq!(missing([cups, usb, usb], false), Some(usb));
    }

    #[test]
    fn the_program_is_found_in_path() {
        let dir = std::env::temp_dir().join(format!("ipp-usb-path-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let expected = dir.to_string_lossy().to_string();
        // an empty directory is not enough
        assert!(!installed_in(Some(OsStr::new(&expected))));
        let program = dir.join(PROGRAM);
        fs::write(&program, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // present but not executable: still missing
            assert!(!installed_in(Some(OsStr::new(&expected))));
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(installed_in(Some(OsStr::new(&expected))));
        }
        #[cfg(not(unix))]
        assert!(installed_in(Some(OsStr::new(&expected))));
        // several entries, and no PATH at all
        let two = std::env::join_paths([std::env::temp_dir(), dir.clone()]).unwrap();
        assert!(installed_in(Some(&two)));
        assert!(!installed_in(None));
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn serving_needs_something_on_the_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(serving_at("127.0.0.1", port));
        drop(listener);
        // the port is free now, but another process may grab it: use a fresh
        // listener to know exactly which port is closed
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        drop(closed);
        assert!(!serving_at("127.0.0.1", closed_port));
    }
    #[test]
    fn a_serial_is_looked_for_in_sysfs() {
        let dir = std::env::temp_dir().join(format!("ipp-usb-sysfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("1-1")).unwrap();
        std::fs::write(dir.join("1-1/serial"), "CN8BE790YY06MD\n").unwrap();
        std::env::set_var(SYSFS_ENV, &dir);
        assert_eq!(attached("CN8BE790YY06MD"), Some(true));
        assert_eq!(attached("SOMETHING-ELSE"), Some(false));
        std::env::remove_var(SYSFS_ENV);
        let _ = std::fs::remove_dir_all(&dir);
        // a directory with no serials at all is "cannot tell", not "absent"
        let empty =
            std::env::temp_dir().join(format!("ipp-usb-sysfs-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        std::env::set_var(SYSFS_ENV, &empty);
        assert_eq!(attached("CN8BE790YY06MD"), None);
        std::env::remove_var(SYSFS_ENV);
        let _ = std::fs::remove_dir_all(&empty);
    }
    #[test]
    fn an_ipp_usb_uri_names_its_port() {
        assert_eq!(
            ipp_usb_port_of("ipp://localhost:60000/ipp/print"),
            Some(60000)
        );
        assert_eq!(
            ipp_usb_port_of("ipp://127.0.0.1:60001/ipp/print"),
            Some(60001)
        );
        assert_eq!(
            ipp_usb_port_of("ipp://localhost:61000/ipp/print"),
            Some(61000)
        );
        // outside the range ipp-usb allocates from
        assert_eq!(ipp_usb_port_of("ipp://localhost:61001/ipp/print"), None);
        // a local CUPS queue is not an ipp-usb endpoint
        assert_eq!(ipp_usb_port_of("ipp://127.0.0.1:631/printers/x"), None);
        // and the path has to be the one ipp-usb publishes
        assert_eq!(ipp_usb_port_of("ipp://localhost:60000/other"), None);
    }
}
