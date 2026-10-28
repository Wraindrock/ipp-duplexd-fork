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

/// The port `ipp-usb` serves IPP on.
pub const PORT: u16 = 60000;

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
}
