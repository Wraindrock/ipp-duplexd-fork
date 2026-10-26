//! The GTK4 front end's logic, kept free of GTK so that it can be tested
//! without a display: argument parsing, the flip prompt's text, and where the
//! dialog helper lives.
//!
//! The window itself lives in `src/bin/ipp-duplexd-fork-gui.rs`, which is only
//! built with `--features gui`; everything here is part of the default build.

use crate::l10n;
use std::path::{Path, PathBuf};

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

/// What the user asked the GUI to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    /// Ask whether the stack has been flipped, then print the backs.
    Flip {
        job: String,
    },
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
        // Nothing to show yet: the manager window comes later, so a bare
        // invocation explains itself rather than opening a blank window.
        (None, None) => Mode::Help,
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
        assert_eq!(args(&[]).unwrap().mode, Mode::Help);
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
