//! The CUPS side of a virtual printer: the queue your desktop lists.
//!
//! A `[[printer]]` in the configuration file makes the daemon listen on an
//! address, but that alone does not put anything in the printer settings of a
//! desktop: those list CUPS queues. So every printer the manager window adds is
//! also registered with CUPS as a driverless queue that points at the daemon,
//! which is exactly what the setup wizard does when it runs `lpadmin`.
//!
//! The commands live here rather than in the window so that the argument lists
//! can be tested without a GTK display or a CUPS server.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::discover;

/// Command used to add, change and remove queues.
pub const LPADMIN: &str = "lpadmin";
/// Command used to ask whether a queue exists.
pub const LPSTAT: &str = "lpstat";

/// Overrides the `lpadmin` program; tests use it to point at a stub.
pub const LPADMIN_ENV: &str = "IPP_DUPLEXD_FORK_LPADMIN";
/// Overrides the `lpstat` program; tests use it to point at a stub.
pub const LPSTAT_ENV: &str = "IPP_DUPLEXD_FORK_LPSTAT";

fn lpadmin() -> String {
    std::env::var(LPADMIN_ENV).unwrap_or_else(|_| LPADMIN.to_string())
}

fn lpstat() -> String {
    std::env::var(LPSTAT_ENV).unwrap_or_else(|_| LPSTAT.to_string())
}

/// The device URI of a virtual printer listening on `addr`.
pub fn queue_uri(addr: &str) -> String {
    format!("ipp://{addr}/ipp/print")
}

/// `lpadmin` arguments that create the queue, or repoint an existing one.
pub fn add_arguments(name: &str, addr: &str) -> Vec<String> {
    ["-p", name, "-E", "-v", &queue_uri(addr), "-m", "everywhere"]
        .iter()
        .map(|part| part.to_string())
        .collect()
}

/// `lpadmin` arguments that delete the queue.
pub fn remove_arguments(name: &str) -> Vec<String> {
    ["-x", name].iter().map(|part| part.to_string()).collect()
}

/// Whether CUPS can be told about queues at all.
pub fn available() -> bool {
    Command::new(lpadmin()).arg("-h").output().is_ok()
}

/// Whether CUPS has a queue called `name`.
pub fn exists(name: &str) -> bool {
    Command::new(lpstat())
        .args(["-p", name])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Run `lpadmin` and keep the message it printed, not just the exit code.
fn run_lpadmin(args: &[String]) -> Result<(), String> {
    let output = Command::new(lpadmin())
        .args(args)
        .output()
        .map_err(|e| format!("{LPADMIN}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let error = String::from_utf8_lossy(&output.stderr);
    let error = error.trim();
    if error.is_empty() {
        return Err(format!("{LPADMIN} exited with {}", output.status));
    }
    Err(error.to_string())
}

/// Register the queue for a virtual printer, or repoint it after its address
/// changed. Idempotent: `lpadmin` modifies an existing queue in place.
pub fn ensure(name: &str, addr: &str) -> Result<(), String> {
    run_lpadmin(&add_arguments(name, addr))?;
    if !exists(name) {
        return Err(format!("{LPSTAT} does not list '{name}' after adding it"));
    }
    Ok(())
}

/// Delete the queue, if CUPS knows it. Returns whether anything was removed.
pub fn remove(name: &str) -> Result<bool, String> {
    if !exists(name) {
        return Ok(false);
    }
    run_lpadmin(&remove_arguments(name))?;
    Ok(true)
}

/// `lpadmin` arguments that turn a queue into a driverless one on `ipp-usb`.
///
/// The same shape as [`add_arguments`], but pointed at ipp-usb rather than at a
/// virtual printer, so a queue that used to go through hplip keeps working.
pub fn driverless_arguments(name: &str, uri: &str) -> Vec<String> {
    ["-p", name, "-E", "-v", uri, "-m", "everywhere"]
        .iter()
        .map(|part| part.to_string())
        .collect()
}

/// `lpadmin` arguments that put a queue back on the device URI and PPD saved by
/// [`switch_to_ipp_usb`].
pub fn restore_arguments(name: &str, device_uri: &str, ppd: &Path) -> Vec<String> {
    [
        "-p",
        name,
        "-E",
        "-v",
        device_uri,
        "-P",
        ppd.to_str().unwrap_or_default(),
    ]
    .iter()
    .map(|part| part.to_string())
    .collect()
}

/// Where the device URI of `queue` is kept before it is switched.
pub fn backup_device(dir: &Path, queue: &str) -> PathBuf {
    dir.join(format!("{queue}.device-uri"))
}

/// Where the PPD of `queue` is kept before it is switched.
pub fn backup_ppd(dir: &Path, queue: &str) -> PathBuf {
    dir.join(format!("{queue}.ppd"))
}

/// Whether a switch of `queue` can still be undone.
pub fn has_backup(dir: &Path, queue: &str) -> bool {
    backup_device(dir, queue).is_file() && backup_ppd(dir, queue).is_file()
}

/// Queues that have something to restore, sorted for a stable listing.
pub fn backups(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter_map(|file| file.strip_suffix(".device-uri").map(str::to_string))
        .filter(|queue| has_backup(dir, queue))
        .collect();
    names.sort();
    names
}

/// Point `queue` at ipp-usb instead of hplip, saving what undoing it needs.
///
/// The backups are written first, so a queue that the switch could not change
/// is still described; if `lpadmin` fails they are removed again rather than
/// leaving a restore entry for something that never happened.
pub fn switch_to_ipp_usb(
    queue: &str,
    device_uri: &str,
    ppd: &str,
    uri: &str,
    dir: &Path,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    fs::write(backup_device(dir, queue), device_uri)
        .map_err(|e| format!("{}: {e}", backup_device(dir, queue).display()))?;
    fs::write(backup_ppd(dir, queue), ppd)
        .map_err(|e| format!("{}: {e}", backup_ppd(dir, queue).display()))?;
    if let Err(e) = run_lpadmin(&driverless_arguments(queue, uri)) {
        let _ = fs::remove_file(backup_device(dir, queue));
        let _ = fs::remove_file(backup_ppd(dir, queue));
        return Err(e);
    }
    if !exists(queue) {
        return Err(format!(
            "{LPSTAT} does not list '{queue}' after switching it"
        ));
    }
    Ok(())
}

/// Put `queue` back on the device URI and PPD saved by [`switch_to_ipp_usb`],
/// then drop the backups. Returns the device URI it went back to.
pub fn restore(queue: &str, dir: &Path) -> Result<String, String> {
    let device = fs::read_to_string(backup_device(dir, queue))
        .map_err(|e| format!("{}: {e}", backup_device(dir, queue).display()))?;
    let device = device.trim().to_string();
    if device.is_empty() {
        return Err(format!(
            "{}: empty device URI",
            backup_device(dir, queue).display()
        ));
    }
    let ppd = backup_ppd(dir, queue);
    if !ppd.is_file() {
        return Err(format!("{}: missing", ppd.display()));
    }
    run_lpadmin(&restore_arguments(queue, &device, &ppd))?;
    if !exists(queue) {
        return Err(format!(
            "{LPSTAT} does not list '{queue}' after restoring it"
        ));
    }
    let _ = fs::remove_file(backup_device(dir, queue));
    let _ = fs::remove_file(&ppd);
    Ok(device)
}

/// The PPD CUPS serves for `queue`, so that a switch can be undone.
pub fn ppd_text(queue: &str) -> Result<String, String> {
    ppd_text_at(discover::CUPS_ADDR, queue)
}

/// [`ppd_text`] aimed at another address, so tests answer it themselves.
pub fn ppd_text_at(addr: &str, queue: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| format!("{addr}: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    write!(
        stream,
        "GET /printers/{queue}.ppd HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut answer = String::new();
    stream
        .read_to_string(&mut answer)
        .map_err(|e| e.to_string())?;
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("{addr}: no HTTP body"))?;
    if !head.starts_with("HTTP/1.") || !head.contains(" 200 ") {
        let status = head.lines().next().unwrap_or("?").to_string();
        return Err(format!("{addr}: {status}"));
    }
    if body.trim().is_empty() {
        return Err(format!("{addr}: empty PPD for '{queue}'"));
    }
    Ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};

    /// The overrides are process-wide, so the tests that set them take turns.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn stub(dir: &std::path::Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "#!/bin/sh\n{body}").unwrap();
        drop(file);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path.display().to_string()
    }

    fn tempdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ipp-queues-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_queue_points_at_the_daemon() {
        assert_eq!(
            queue_uri("127.0.0.1:6632"),
            "ipp://127.0.0.1:6632/ipp/print"
        );
        assert_eq!(
            queue_uri("192.168.1.20:6633"),
            "ipp://192.168.1.20:6633/ipp/print"
        );
    }

    #[test]
    fn adding_names_the_queue_and_the_model() {
        assert_eq!(
            add_arguments("laser-duplex", "127.0.0.1:6632"),
            vec![
                "-p",
                "laser-duplex",
                "-E",
                "-v",
                "ipp://127.0.0.1:6632/ipp/print",
                "-m",
                "everywhere"
            ]
        );
    }

    #[test]
    fn removing_deletes_the_queue() {
        assert_eq!(remove_arguments("laser-duplex"), vec!["-x", "laser-duplex"]);
    }

    #[test]
    fn a_missing_lpadmin_is_reported() {
        let _guard = guard();
        std::env::set_var(LPADMIN_ENV, "/nonexistent/lpadmin");
        let error = ensure("laser-duplex", "127.0.0.1:6632").unwrap_err();
        std::env::remove_var(LPADMIN_ENV);
        assert!(error.contains("lpadmin"), "unexpected error: {error}");
    }

    #[test]
    fn a_refusal_keeps_its_message() {
        let _guard = guard();
        let dir = tempdir("refuse");
        let stub = stub(
            &dir,
            "lpadmin",
            "echo 'lpadmin: Unable to connect to server' >&2\nexit 1",
        );
        std::env::set_var(LPADMIN_ENV, &stub);
        let error = ensure("laser-duplex", "127.0.0.1:6632").unwrap_err();
        std::env::remove_var(LPADMIN_ENV);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            error.contains("Unable to connect to server"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_queue_that_does_not_show_up_is_an_error() {
        let _guard = guard();
        let dir = tempdir("silent");
        let lpadmin = stub(&dir, "lpadmin", "exit 0");
        let lpstat = stub(&dir, "lpstat", "exit 1");
        std::env::set_var(LPADMIN_ENV, &lpadmin);
        std::env::set_var(LPSTAT_ENV, &lpstat);
        let error = ensure("laser-duplex", "127.0.0.1:6632").unwrap_err();
        std::env::remove_var(LPADMIN_ENV);
        std::env::remove_var(LPSTAT_ENV);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(error.contains("laser-duplex"), "unexpected error: {error}");
    }

    #[test]
    fn removing_a_queue_cups_never_had_is_not_an_error() {
        let _guard = guard();
        let dir = tempdir("absent");
        let lpstat = stub(&dir, "lpstat", "exit 1");
        std::env::set_var(LPSTAT_ENV, &lpstat);
        let removed = remove("laser-duplex").unwrap();
        std::env::remove_var(LPSTAT_ENV);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!removed);
    }

    /// A stub `lpadmin` that appends its arguments to `log` and then answers
    /// with `exit`.
    fn recording_lpadmin(dir: &std::path::Path, log: &std::path::Path, exit: &str) -> String {
        let body = format!("echo \"$@\" >> '{}'\n{}", log.display(), exit);
        stub(dir, "lpadmin", &body)
    }

    #[test]
    fn switching_saves_the_old_uri_and_ppd() {
        let _guard = guard();
        let dir = tempdir("switch");
        let backup = dir.join("backup");
        let log = dir.join("lpadmin-args");
        let lpadmin = recording_lpadmin(&dir, &log, "exit 0");
        let lpstat = stub(&dir, "lpstat", "exit 0");
        std::env::set_var(LPADMIN_ENV, &lpadmin);
        std::env::set_var(LPSTAT_ENV, &lpstat);

        switch_to_ipp_usb(
            "DeskJet-2600",
            "hp:/usb/DeskJet_2600_series?serial=X",
            "%!PS-Adobe-3.0 PPD",
            "ipp://localhost:60001/ipp/print",
            &backup,
        )
        .unwrap();

        std::env::remove_var(LPADMIN_ENV);
        std::env::remove_var(LPSTAT_ENV);

        assert!(has_backup(&backup, "DeskJet-2600"));
        assert_eq!(
            std::fs::read_to_string(backup_device(&backup, "DeskJet-2600")).unwrap(),
            "hp:/usb/DeskJet_2600_series?serial=X"
        );
        assert_eq!(
            std::fs::read_to_string(backup_ppd(&backup, "DeskJet-2600")).unwrap(),
            "%!PS-Adobe-3.0 PPD"
        );
        let args = std::fs::read_to_string(&log).unwrap();
        assert!(args.contains("ipp://localhost:60001/ipp/print"), "{args}");
        assert!(args.contains("-m everywhere"), "{args}");
        assert_eq!(backups(&backup), vec!["DeskJet-2600".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rejected_switch_leaves_nothing_to_restore() {
        let _guard = guard();
        let dir = tempdir("switch-fail");
        let backup = dir.join("backup");
        let log = dir.join("lpadmin-args");
        let lpadmin = recording_lpadmin(&dir, &log, "echo 'lpadmin: Forbidden' >&2\nexit 1");
        std::env::set_var(LPADMIN_ENV, &lpadmin);
        let error = switch_to_ipp_usb(
            "DeskJet-2600",
            "hp:/usb/x",
            "ppd",
            "ipp://localhost:60000/ipp/print",
            &backup,
        )
        .unwrap_err();
        std::env::remove_var(LPADMIN_ENV);
        assert!(error.contains("Forbidden"), "unexpected error: {error}");
        assert!(!has_backup(&backup, "DeskJet-2600"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restoring_puts_the_saved_device_and_ppd_back() {
        let _guard = guard();
        let dir = tempdir("restore");
        let backup = dir.join("backup");
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(
            backup_device(&backup, "DeskJet-2600"),
            "hp:/usb/DeskJet_2600_series?serial=X\n",
        )
        .unwrap();
        std::fs::write(backup_ppd(&backup, "DeskJet-2600"), "PPD").unwrap();

        let log = dir.join("lpadmin-args");
        let lpadmin = recording_lpadmin(&dir, &log, "exit 0");
        let lpstat = stub(&dir, "lpstat", "exit 0");
        std::env::set_var(LPADMIN_ENV, &lpadmin);
        std::env::set_var(LPSTAT_ENV, &lpstat);
        let device = restore("DeskJet-2600", &backup).unwrap();
        std::env::remove_var(LPADMIN_ENV);
        std::env::remove_var(LPSTAT_ENV);

        assert_eq!(device, "hp:/usb/DeskJet_2600_series?serial=X");
        let args = std::fs::read_to_string(&log).unwrap();
        assert!(
            args.contains("hp:/usb/DeskJet_2600_series?serial=X"),
            "{args}"
        );
        assert!(args.contains("-P"), "{args}");
        // the backups are gone once they were used
        assert!(!has_backup(&backup, "DeskJet-2600"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ppd_is_fetched_from_cups() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // read the whole request header, however many segments it arrives in
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                match std::io::Read::read(&mut stream, &mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => request.push(byte[0]),
                }
            }
            let ppd = "*PPD-Adobe: \"4.3\"\n";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                ppd.len()
            );
            std::io::Write::write_all(&mut stream, head.as_bytes()).unwrap();
            std::io::Write::write_all(&mut stream, ppd.as_bytes()).unwrap();
        });
        let ppd = ppd_text_at(&addr, "DeskJet-2600").unwrap();
        server.join().unwrap();
        assert_eq!(ppd, "*PPD-Adobe: \"4.3\"\n");
    }
}
