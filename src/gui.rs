//! The GTK4 front end's logic, kept free of GTK so that it can be tested
//! without a display: argument parsing, the flip prompt's text, and where the
//! dialog helper lives.
//!
//! The window itself lives in `src/bin/ipp-duplexd-fork-gui.rs`, which is only
//! built with `--features gui`; everything here is part of the default build.

use crate::l10n;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Product name. Never translated: it is the window title and the icon name.
pub const NAME: &str = "ipp-duplexd-fork";

/// The helper binary the daemon looks for when it wants a dialog.
pub const BIN: &str = "ipp-duplexd-fork-gui";
/// The reverse-DNS name GTK and libadwaita register the dialog under; it also
/// gives the window the right icon and lets the desktop put it in its own
/// window group.
pub const APP_ID: &str = "io.github.wraindrock.ipp_duplexd_fork";

/// Exit codes the daemon reads: it only ever turns zero into a flip.
pub const EXIT_ACCEPT: u8 = 0;
pub const EXIT_DECLINE: u8 = 1;

/// A GUI that cannot open a display must never look like a confirmation, and
/// must say why instead of hanging: the daemon keeps waiting for `GET /flip`.
pub const EXIT_NO_DISPLAY: u8 = 2;

/// Where the daemon listens unless it was told otherwise; the window's address
/// row starts here.
pub const DEFAULT_DAEMON: &str = "127.0.0.1:6632";

/// What the user asked the GUI to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    /// Ask whether the stack has been flipped, then print the backs.
    Flip {
        job: String,
    },
    /// No arguments: open the desktop window (status, and the button that
    /// continues a waiting job).
    Window,
    Help,
    Version,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    pub mode: Mode,
    pub lang: &'static str,
    /// Only set by the tests: the dialog answers itself so that the real
    /// widget code can run under a headless X server.
    pub answer: Option<u8>,
}

/// The test hook that makes the dialog answer itself, for the Xvfb smoke test:
/// `IPP_DUPLEXD_FORK_GUI_TEST=accept|decline`.
pub const TEST_ENV: &str = "IPP_DUPLEXD_FORK_GUI_TEST";

/// `accept` / `decline` / anything else (the hook is deliberately not a flag,
/// so it cannot be mistaken for something a user is supposed to type).
pub fn test_answer(raw: Option<&str>) -> Option<u8> {
    match raw {
        Some("accept") => Some(EXIT_ACCEPT),
        Some("decline") => Some(EXIT_DECLINE),
        _ => None,
    }
}

/// `IPP_DUPLEXD_FORK_GUI_TEST=window` makes the window close itself at once,
/// so the Xvfb smoke test can prove that a bare invocation really opens one.
pub fn window_test(raw: Option<&str>) -> bool {
    raw == Some("window")
}

/// The same hook, but for the "+" dialog: the smoke test opens it so the
/// detection list is built for real instead of only being compiled.
pub fn add_test(raw: Option<&str>) -> bool {
    raw == Some("add")
}

/// `IPP_DUPLEXD_FORK_GUI_TEST=setup` presses the first-run "Set up" button by
/// itself, so a headless machine can prove that the window installs the
/// service (with a stub `systemctl` and a temporary HOME) instead of only
/// compiling the code that would.
pub fn setup_test(raw: Option<&str>) -> bool {
    raw == Some("setup")
}

/// Parse the helper's command line. `default_lang` is the environment's choice
/// (passed in rather than read here, so tests are not at the mercy of the
/// environment they run in).
pub fn parse_args(
    argv: impl IntoIterator<Item = String>,
    default_lang: &'static str,
    answer: Option<u8>,
) -> Result<Args, String> {
    let mut argv = argv.into_iter();
    let mut lang = default_lang;
    let mut flip_job: Option<String> = None;
    let mut mode: Option<Mode> = None;
    while let Some(arg) = argv.next() {
        let mut value = |what: &str| {
            argv.next().ok_or_else(|| {
                l10n::tr(
                    l10n::FALLBACK,
                    "gui.err.needs_value",
                    &[("what", what.to_string())],
                )
            })
        };
        match arg.as_str() {
            "--flip" => flip_job = Some(value("--flip")?),
            "--lang" => {
                let asked = value("--lang")?;
                lang = l10n::normalize(&asked).ok_or_else(|| {
                    l10n::tr(
                        l10n::FALLBACK,
                        "gui.err.bad_lang",
                        &[("lang", asked), ("list", l10n::describe_all())],
                    )
                })?;
            }
            "--help" | "-h" => mode = Some(Mode::Help),
            "--version" | "-V" => mode = Some(Mode::Version),
            other => {
                return Err(l10n::tr(
                    l10n::FALLBACK,
                    "gui.err.unknown_arg",
                    &[("arg", other.to_string())],
                ))
            }
        }
    }
    let mode = match (mode, flip_job) {
        (Some(mode), _) => mode,
        (None, Some(job)) => Mode::Flip { job },
        // No arguments means "start the program": a desktop application
        // opens its window. `--help`/`--version` stay explicit.
        (None, None) => Mode::Window,
    };
    Ok(Args { mode, lang, answer })
}

/// Everything the dialog shows, already in the user's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlipPrompt {
    pub text: String,
    pub accept: String,
    pub decline: String,
}

pub fn flip_prompt(locale: &str, job: &str) -> FlipPrompt {
    FlipPrompt {
        text: l10n::tr(locale, "daemon.dialog.text", &[("job", job.to_string())]),
        accept: l10n::text(locale, "daemon.dialog.ok"),
        decline: l10n::text(locale, "daemon.dialog.not_yet"),
    }
}

/// The words the main window uses, in the user's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowLabels {
    pub title: String,
    pub daemon_group: String,
    pub address_title: String,
    pub address_subtitle: String,
    pub status_title: String,
    pub refresh: String,
    pub flip_title: String,
    pub flip_subtitle: String,
    pub about_group: String,
    pub version_title: String,
    pub printers_title: String,
    pub add_tooltip: String,
    pub edit_tooltip: String,
    pub remove_tooltip: String,
    pub flip_tooltip: String,
    pub service_title: String,
    pub restart: String,
    pub settings_title: String,
    pub language_title: String,
    pub config_title: String,
    pub no_printers: String,
    pub no_printers_body: String,
    pub dialog_add: String,
    pub dialog_edit: String,
    pub field_name: String,
    pub field_listen: String,
    pub field_real: String,
    pub save: String,
    pub cancel: String,
    pub remove_title: String,
    pub remove_body: String,
    pub remove: String,
    pub saved: String,
    pub save_failed: String,
    pub restarted: String,
    pub restart_failed: String,
    pub service_missing: String,
    pub detected_title: String,
    pub detected_hint: String,
    pub detected_none: String,
    pub detected_failed: String,
    pub advanced: String,
    pub advanced_hint: String,
    pub setup_title: String,
    pub setup_body: String,
    pub setup_button: String,
    pub setup_done: String,
    pub setup_failed: String,
    pub setup_no_answer: String,
    pub service_not_installed: String,
}

pub fn window_labels(locale: &str) -> WindowLabels {
    let text = |key: &str| l10n::text(locale, key);
    WindowLabels {
        title: NAME.to_string(),
        daemon_group: text("gui.window.daemon_group"),
        address_title: text("gui.window.address_title"),
        address_subtitle: text("gui.window.address_subtitle"),
        status_title: text("gui.window.status_title"),
        refresh: text("gui.window.refresh"),
        flip_title: text("gui.window.flip_title"),
        flip_subtitle: text("gui.window.flip_subtitle"),
        about_group: text("gui.window.about_group"),
        version_title: text("gui.window.version_title"),
        printers_title: text("gui.window.printers_title"),
        add_tooltip: text("gui.window.add_tooltip"),
        edit_tooltip: text("gui.window.edit_tooltip"),
        remove_tooltip: text("gui.window.remove_tooltip"),
        flip_tooltip: text("gui.window.flip_tooltip"),
        service_title: text("gui.window.service_title"),
        restart: text("gui.window.restart"),
        settings_title: text("gui.window.settings_title"),
        language_title: text("gui.window.language_title"),
        config_title: text("gui.window.config_title"),
        no_printers: text("gui.window.no_printers"),
        no_printers_body: text("gui.window.no_printers_body"),
        dialog_add: text("gui.window.dialog_add"),
        dialog_edit: text("gui.window.dialog_edit"),
        field_name: text("gui.window.field_name"),
        field_listen: text("gui.window.field_listen"),
        field_real: text("gui.window.field_real"),
        save: text("common.save"),
        cancel: text("common.cancel"),
        remove_title: text("gui.window.remove_title"),
        remove_body: text("gui.window.remove_body"),
        remove: text("common.remove"),
        saved: text("gui.window.saved"),
        save_failed: text("gui.window.save_failed"),
        restarted: text("gui.window.restarted"),
        restart_failed: text("gui.window.restart_failed"),
        service_missing: text("gui.window.service_missing"),
        detected_title: text("gui.window.detected_title"),
        detected_hint: text("gui.window.detected_hint"),
        detected_none: text("gui.window.detected_none"),
        detected_failed: text("gui.window.detected_failed"),
        advanced: text("gui.window.advanced"),
        advanced_hint: text("gui.window.advanced_hint"),
        setup_title: text("gui.window.setup_title"),
        setup_body: text("gui.window.setup_body"),
        setup_button: text("gui.window.setup_button"),
        setup_done: text("gui.window.setup_done"),
        setup_failed: text("gui.window.setup_failed"),
        setup_no_answer: text("gui.window.setup_no_answer"),
        service_not_installed: text("gui.window.service_not_installed"),
    }
}

/// The next free loopback address for a new virtual printer. The manager
/// starts at the documented default and walks upwards, so adding a second
/// printer does not collide with the first one.
pub fn default_listen(used: &[String]) -> String {
    for offset in 0..64 {
        let candidate = format!("127.0.0.1:{}", 6632 + offset);
        if !used.contains(&candidate) {
            return candidate;
        }
    }
    format!(
        "127.0.0.1:{}",
        DEFAULT_DAEMON.rsplit(':').next().unwrap_or("6632")
    )
}

/// Turn a discovered CUPS queue into a virtual printer: a unique name, a free
/// loopback address, and the queue's URI as the printer to forward to. The
/// window calls this when the user picks a row from the detected list.
pub fn printer_from_detected(
    detected: &crate::discover::Discovered,
    existing_names: &[String],
    used_listens: &[String],
) -> crate::config::Printer {
    crate::config::Printer::new(
        &crate::discover::suggested_name(&detected.queue, existing_names),
        &default_listen(used_listens),
        &detected.uri,
    )
}

/// The printers that belong in CUPS: those whose daemon answers. `lpadmin -m
/// everywhere` asks the printer what it can do, so a printer that is not
/// listening yet cannot be registered — it is tried again the next time the
/// window opens or a change is saved.
pub fn queues_to_register<Answers: Fn(&str) -> bool>(
    printers: &[crate::config::Printer],
    answers: Answers,
) -> Vec<crate::config::Printer> {
    printers
        .iter()
        .filter(|printer| answers(&printer.listen))
        .cloned()
        .collect()
}

/// "127.0.0.1:6632 → ipp://127.0.0.1:631/printers/laser", for a printer row.
pub fn row_detail(locale: &str, listen: &str, real: &str) -> String {
    l10n::tr(
        locale,
        "gui.window.row_detail",
        &[("listen", listen.to_string()), ("real", real.to_string())],
    )
}

/// POST with an empty body — the daemon's `/reload`.
pub fn http_post(addr: &str, path: &str, lang: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nAccept-Language: {lang}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = match text.split_once("\r\n\r\n") {
        Some(parts) => parts,
        None => return Err("the daemon sent no complete HTTP response".to_string()),
    };
    let status = head.lines().next().unwrap_or_default();
    if !status.contains(" 200") {
        return Err(format!("the daemon answered {status}"));
    }
    Ok(body.trim().to_string())
}

/// Tell every listening printer to re-read the configuration file. Returns how
/// many answered; zero means the daemon is not running.
pub fn reload_everywhere(addrs: &[String], lang: &str) -> usize {
    addrs
        .iter()
        .filter(|addr| http_post(addr, "/reload", lang).is_ok())
        .count()
}

pub fn unreachable(locale: &str, addr: &str, err: &str) -> String {
    l10n::tr(
        locale,
        "gui.window.unreachable",
        &[("addr", addr.to_string()), ("err", err.to_string())],
    )
}

/// A minimal HTTP/1.1 GET, enough to ask the daemon for its status page and to
/// trigger `GET /flip`. The daemon answers in the language of the
/// `Accept-Language` header, so the window shows its own language instead of
/// translating the daemon's words twice.
pub fn http_get(addr: &str, path: &str, lang: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nAccept-Language: {lang}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| "short response".to_string())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| "no status line".to_string())?;
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    Ok(body.to_string())
}

/// The helper next to the running binary, so a `cargo build` tree and an
/// installed `/usr/bin` both find it.
pub fn find_helper(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let candidate = dir.join(BIN);
    candidate.is_file().then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Args, String> {
        parse_args(list.iter().map(|s| s.to_string()), l10n::FALLBACK, None)
    }

    #[test]
    fn a_detected_queue_becomes_a_complete_printer() {
        let detected = crate::discover::Discovered {
            queue: "Office Laser".to_string(),
            model: "HP DeskJet 2600".to_string(),
            uri: "ipp://127.0.0.1:631/printers/Office Laser".to_string(),
            state: Some(3),
        };
        let printer = printer_from_detected(
            &detected,
            &["office-laser-duplex".to_string()],
            &["127.0.0.1:6632".to_string()],
        );
        assert_eq!(printer.name, "office-laser-duplex-2");
        assert_eq!(printer.listen, "127.0.0.1:6633");
        assert_eq!(printer.real, "ipp://127.0.0.1:631/printers/Office Laser");
    }

    #[test]
    fn only_printers_that_answer_are_registered_with_cups() {
        let alpha = crate::config::Printer::new(
            "alpha",
            "127.0.0.1:6632",
            "ipp://127.0.0.1:631/printers/laser",
        );
        let beta = crate::config::Printer::new(
            "beta",
            "127.0.0.1:6633",
            "ipp://127.0.0.1:631/printers/inkjet",
        );
        let printers = vec![alpha.clone(), beta];
        let answering = |addr: &str| addr == "127.0.0.1:6632";
        let queued = queues_to_register(&printers, answering);
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].name, "alpha");
        assert_eq!(queued[0].listen, "127.0.0.1:6632");
    }

    #[test]
    fn a_new_printer_gets_the_next_free_loopback_address() {
        assert_eq!(default_listen(&[]), "127.0.0.1:6632");
        assert_eq!(
            default_listen(&["127.0.0.1:6632".to_string()]),
            "127.0.0.1:6633"
        );
        assert_eq!(
            default_listen(&["127.0.0.1:6632".to_string(), "127.0.0.1:6633".to_string()]),
            "127.0.0.1:6634"
        );
        // An address nobody uses is still not offered twice.
        assert_eq!(
            default_listen(&["192.0.2.7:6632".to_string()]),
            "127.0.0.1:6632"
        );
    }

    #[test]
    fn flip_mode_takes_the_job_name() {
        let parsed = args(&["--flip", "report [odd]"]).unwrap();
        assert_eq!(
            parsed.mode,
            Mode::Flip {
                job: "report [odd]".to_string()
            }
        );
    }

    #[test]
    fn the_language_flag_is_the_same_spelling_as_elsewhere() {
        assert_eq!(
            args(&["--flip", "x", "--lang", "zh_CN.UTF-8"])
                .unwrap()
                .lang,
            "zh-CN"
        );
        assert_eq!(
            args(&["--flip", "x", "--lang", "zh-TW"]).unwrap().lang,
            "zh-CN"
        );
        assert_eq!(args(&["--flip", "x"]).unwrap().lang, l10n::FALLBACK);
        assert_eq!(
            parse_args(["--flip".to_string(), "x".to_string()], "zh-CN", None)
                .unwrap()
                .lang,
            "zh-CN"
        );
    }

    #[test]
    fn a_bad_language_lists_the_known_ones() {
        let err = args(&["--lang", "de-DE"]).unwrap_err();
        assert!(err.contains("de-DE"), "{err}");
        assert!(err.contains("zh-CN"), "{err}");
    }

    #[test]
    fn a_missing_value_and_an_unknown_flag_are_errors() {
        assert!(args(&["--flip"]).unwrap_err().contains("--flip"));
        assert!(args(&["--nope"]).unwrap_err().contains("--nope"));
    }

    #[test]
    fn help_and_version_win_over_flip() {
        assert_eq!(args(&["--flip", "x", "--help"]).unwrap().mode, Mode::Help);
        assert_eq!(args(&["--version"]).unwrap().mode, Mode::Version);
    }

    #[test]
    fn no_arguments_opens_the_window_without_asking_for_help() {
        // A desktop program started from the menu must open its window; it
        // must not answer with a usage text in the terminal.
        assert_eq!(args(&[]).unwrap().mode, Mode::Window);
        assert_eq!(args(&["--lang", "zh-CN"]).unwrap().mode, Mode::Window);
    }

    #[test]
    fn the_window_labels_exist_in_both_languages() {
        let en = window_labels("en");
        assert!(en.flip_title.contains("backs"), "{}", en.flip_title);
        assert!(en.status_title.contains("Status"), "{}", en.status_title);
        assert_eq!(en.title, NAME);

        let zh = window_labels("zh-CN");
        assert!(zh.flip_title.contains("背面"), "{}", zh.flip_title);
        assert_ne!(zh.flip_title, en.flip_title);
        assert!(unreachable("zh-CN", "127.0.0.1:1", "refused").contains("127.0.0.1:1"));
    }

    #[test]
    fn the_status_request_names_the_language_it_wants() {
        use std::net::TcpListener;
        use std::sync::mpsc;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let read = stream.read(&mut request).unwrap();
            sender
                .send(String::from_utf8_lossy(&request[..read]).into_owned())
                .unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello")
                .unwrap();
        });

        let body = http_get(&addr.to_string(), "/", "zh-CN").unwrap();
        assert_eq!(body, "hello");
        let request = receiver.recv().unwrap();
        assert!(request.starts_with("GET / HTTP/1.1"), "{request}");
        assert!(request.contains("Accept-Language: zh-CN"), "{request}");
    }

    #[test]
    fn a_refusing_daemon_is_an_error_not_a_status() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(b"HTTP/1.1 405 Method Not Allowed\r\n\r\nnope");
        });
        assert!(http_get(&addr.to_string(), "/", "en").is_err());
    }

    #[test]
    fn the_window_hook_is_its_own_word() {
        assert!(window_test(Some("window")));
        assert!(!window_test(Some("accept")));
        assert!(!window_test(None));
        assert!(add_test(Some("add")));
        assert!(!add_test(Some("window")));
    }

    #[test]
    fn the_test_hook_only_answers_known_words() {
        assert_eq!(test_answer(Some("accept")), Some(EXIT_ACCEPT));
        assert_eq!(test_answer(Some("decline")), Some(EXIT_DECLINE));
        assert_eq!(test_answer(Some("yes")), None);
        assert_eq!(test_answer(None), None);
    }

    #[test]
    fn the_prompt_speaks_the_requested_language() {
        let en = flip_prompt("en", "report");
        assert!(en.accept.contains("OK"), "{}", en.accept);
        assert!(en.text.contains("report"));

        let zh = flip_prompt("zh-CN", "报告");
        assert!(zh.accept.contains("打印背面"), "{}", zh.accept);
        assert!(zh.text.contains("报告"));
        assert_ne!(zh.decline, en.decline);
    }

    #[test]
    fn the_helper_is_looked_for_next_to_the_binary() {
        let dir = std::env::temp_dir().join(format!("ipd-gui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("ipp-duplexd-fork");
        assert_eq!(find_helper(&exe), None);
        let helper = dir.join(BIN);
        std::fs::write(&helper, b"#!/bin/sh\n").unwrap();
        assert_eq!(find_helper(&exe), Some(helper));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
