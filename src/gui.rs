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
    }
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
