//! The GTK4 dialog's contract with the daemon, checked without a display.
//!
//! The dialog binary only exists when the crate is built with `--features
//! gui`, so every test here skips itself in a plain `cargo test` (CI builds
//! this file both ways: the default job skips, the GTK4 job runs the tests).
//!
//! What matters to the daemon is the exit code, so that is what is asserted:
//! zero means "the stack is flipped, print the backs", anything else leaves
//! the job waiting for `GET /flip`.

#![cfg(feature = "gui")]

use std::path::PathBuf;
use std::process::{Command, Output};

fn run(args: &[&str], drop_display: bool) -> Option<Output> {
    // Cargo defines the variable for the target even when `required-features`
    // skipped building it, so check that the file is really there.
    let bin: PathBuf = option_env!("CARGO_BIN_EXE_ipp-duplexd-fork-gui")?.into();
    if !bin.is_file() {
        return None;
    }
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("GDK_BACKEND", "x11");
    if !drop_display {
        for var in ["DISPLAY", "WAYLAND_DISPLAY"] {
            if let Ok(value) = std::env::var(var) {
                cmd.env(var, value);
            }
        }
    } else {
        // A display number nobody listens on: GTK fails cleanly instead of
        // probing every backend (and, on a broken runtime, crashing).
        cmd.env("DISPLAY", ":44");
    }
    Some(cmd.output().expect("run the dialog"))
}

#[test]
fn the_dialog_explains_itself() {
    let Some(out) = run(&["--help"], false) else {
        return;
    };
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--flip"), "{text}");
    assert!(text.contains("ipp-duplexd-fork-gui"), "{text}");

    let Some(zh) = run(&["--lang", "zh-CN", "--help"], false) else {
        return;
    };
    assert_eq!(zh.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&zh.stdout).contains("用法"));
}

#[test]
fn a_rejected_language_lists_the_known_ones() {
    let Some(out) = run(&["--flip", "demo", "--lang", "de-DE"], false) else {
        return;
    };
    assert_ne!(out.status.code(), Some(0));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("de-DE"), "{err}");
    assert!(err.contains("zh-CN"), "{err}");
}

#[test]
fn without_a_display_the_answer_is_two_and_not_a_confirmation() {
    let Some(out) = run(&["--flip", "demo [odd pages]"], true) else {
        return;
    };
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The daemon uses this hint in its own log; the dialog must tell the user
    // where to confirm instead.
    assert!(String::from_utf8_lossy(&out.stderr).contains("GET /flip"));
}
