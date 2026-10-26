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

use std::process::Command;

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

/// Register the queue for a virtual printer, or repoint it after its address
/// changed. Idempotent: `lpadmin` modifies an existing queue in place.
pub fn ensure(name: &str, addr: &str) -> Result<(), String> {
    let args = add_arguments(name, addr);
    let output = Command::new(lpadmin())
        .args(&args)
        .output()
        .map_err(|e| format!("{LPADMIN}: {e}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        let error = error.trim();
        if error.is_empty() {
            return Err(format!("{LPADMIN} exited with {}", output.status));
        }
        return Err(error.to_string());
    }
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
    let args = remove_arguments(name);
    let output = Command::new(lpadmin())
        .args(&args)
        .output()
        .map_err(|e| format!("{LPADMIN}: {e}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        let error = error.trim();
        if error.is_empty() {
            return Err(format!("{LPADMIN} exited with {}", output.status));
        }
        return Err(error.to_string());
    }
    Ok(true)
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
}
