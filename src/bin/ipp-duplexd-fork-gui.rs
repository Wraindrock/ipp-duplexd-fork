//! The GTK4 front end. Right now it is one window: the "flip the stack" dialog
//! the daemon opens when the odd pages are done. The manager window (printers,
//! jobs, settings) will be built on top of the same binary.
//!
//! The window is an [libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/)
//! `AlertDialog`, not a hand-built `gtk::Window`: libadwaita is GNOME's
//! GTK4 application framework, so the dialog gets the platform look, follows
//! the system light/dark colour scheme automatically (an `AdwApplication`
//! installs an `AdwStyleManager` whose default scheme tracks the desktop's
//! `color-scheme` setting — no code here has to ask) and stays RTL-correct.
//! All of its strings come from rust-i18n, like the daemon's.
//!
//! The daemon runs it as
//!
//! ```text
//! ipp-duplexd-fork-gui --flip "report.pdf [odd pages]" --lang zh-CN
//! ```
//!
//! and reads the exit code: 0 means "the stack is flipped, print the backs",
//! 1 means "not yet" and leaves the job waiting for `GET /flip`, and 2 means
//! the dialog could not run at all (no display) so the daemon must not treat
//! the outcome as a confirmation.

use adw::prelude::*;
use gtk::glib;
use gtk4 as gtk;
use ipp_duplexd_fork::gui::{self, Mode};
use ipp_duplexd_fork::l10n;
use libadwaita as adw;
use rust_i18n::t;
use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

rust_i18n::i18n!("locales", fallback = "en");

/// libadwaita response identifiers; the labels shown on the buttons are
/// translated and only the identifiers are fixed.
const DECLINE: &str = "decline";
const ACCEPT: &str = "accept";

fn main() -> ExitCode {
    let default_lang = l10n::from_env().unwrap_or(l10n::FALLBACK);
    let answer = gui::test_answer(std::env::var(gui::TEST_ENV).ok().as_deref());
    let args = match gui::parse_args(std::env::args().skip(1), default_lang, answer) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{}: {e}", gui::NAME);
            return ExitCode::from(gui::EXIT_DECLINE);
        }
    };
    l10n::apply(args.lang);
    match args.mode {
        Mode::Help => {
            print!("{}", t!("gui.usage", locale = args.lang));
            ExitCode::SUCCESS
        }
        Mode::Version => {
            println!("{} {}", gui::NAME, env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Mode::Flip { job } => {
            let prompt = gui::flip_prompt(args.lang, &job);
            match run_flip_dialog(&prompt, args.answer, args.lang) {
                Ok(code) => ExitCode::from(code),
                Err(e) => {
                    eprintln!("{}: {e}", gui::NAME);
                    ExitCode::from(gui::EXIT_NO_DISPLAY)
                }
            }
        }
    }
}

/// Shows the dialog and returns its answer. Errors only ever mean "no
/// display"; a closed window is a plain "not yet".
fn run_flip_dialog(
    prompt: &gui::FlipPrompt,
    answer: Option<u8>,
    locale: &str,
) -> Result<u8, String> {
    // libadwaita initialises GTK itself; this is the one call that must
    // succeed for a window to exist.
    adw::init().map_err(|e| l10n::tr(locale, "gui.err.no_display", &[("err", e.to_string())]))?;

    let result = Rc::new(Cell::new(gui::EXIT_DECLINE));
    let app = adw::Application::builder()
        .application_id(gui::APP_ID)
        // Two jobs can be waiting at once, and a dialog does not need D-Bus
        // registration to be useful.
        .flags(adw::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    {
        let prompt = prompt.clone();
        let result = result.clone();
        app.connect_activate(move |app| {
            let dialog = adw::AlertDialog::builder()
                .heading(gui::NAME)
                .body(&prompt.text)
                // Escape and the window close button are "not yet", never a
                // confirmation.
                .close_response(DECLINE)
                .default_response(ACCEPT)
                .build();
            dialog.add_response(DECLINE, &prompt.decline);
            dialog.add_response(ACCEPT, &prompt.accept);
            dialog.set_response_appearance(ACCEPT, adw::ResponseAppearance::Suggested);

            let finish = {
                let result = result.clone();
                let app = app.clone();
                Rc::new(move |code: u8| {
                    result.set(code);
                    app.quit();
                })
            };

            {
                let finish = finish.clone();
                let dialog = dialog.clone();
                dialog.connect_response(None, move |_, response| {
                    finish(if response == ACCEPT {
                        gui::EXIT_ACCEPT
                    } else {
                        gui::EXIT_DECLINE
                    });
                });
            }

            if let Some(code) = answer {
                // The headless smoke test drives the real response path.
                let finish = finish.clone();
                glib::timeout_add_local_once(Duration::from_millis(300), move || finish(code));
            }

            dialog.present(None::<&gtk::Window>);
        });
    }

    app.run_with_args::<&str>(&[]);
    Ok(result.get())
}
