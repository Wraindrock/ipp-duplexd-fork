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
use ipp_duplexd_fork::config as cfg_file;
use ipp_duplexd_fork::discover;
use ipp_duplexd_fork::gui::{self, Mode};
use ipp_duplexd_fork::l10n;
use ipp_duplexd_fork::queues;
use ipp_duplexd_fork::service;
use ipp_duplexd_fork::usb;
use libadwaita as adw;
use rust_i18n::t;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::rc::Rc;
use std::time::Duration;

rust_i18n::i18n!("locales", fallback = "en");

/// libadwaita response identifiers; the labels shown on the buttons are
/// translated and only the identifiers are fixed.
const DECLINE: &str = "decline";
const ACCEPT: &str = "accept";

/// What the window should do by itself, for the Xvfb smoke tests. One per
/// value of `IPP_DUPLEXD_FORK_GUI_TEST`; a real run has none of them.
#[derive(Debug, Clone, Copy, Default)]
struct Hooks {
    window: bool,
    add: bool,
    setup: bool,
    uninstall: bool,
}

fn main() -> ExitCode {
    let default_lang = l10n::from_env().unwrap_or(l10n::FALLBACK);
    let hook = std::env::var(gui::TEST_ENV).ok();
    let answer = gui::test_answer(hook.as_deref());
    let hooks = Hooks {
        window: gui::window_test(hook.as_deref()),
        add: gui::add_test(hook.as_deref()),
        setup: gui::setup_test(hook.as_deref()),
        uninstall: gui::uninstall_test(hook.as_deref()),
    };
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
        Mode::Window => match run_window(args.lang, hooks) {
            Ok(code) => ExitCode::from(code),
            Err(e) => {
                eprintln!("{}: {e}", gui::NAME);
                ExitCode::from(gui::EXIT_NO_DISPLAY)
            }
        },
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

/// The manager window: starting the program with no arguments lands here.
///
/// It is the only place the user needs: every virtual printer is a row (print
/// the backs, look at its status, edit it, remove it), the head bar adds one,
/// and the settings pages hold the language, the service and the file the
/// daemon reads. Nothing here is drawn by hand — a `PreferencesPage` inside a
/// `ToolbarView` in a `ToastOverlay`, all stock libadwaita widgets, so the
/// window follows the desktop's light/dark scheme, accent colour and fonts.
///
/// Every change goes the same way: edit the in-memory configuration, validate
/// the candidate, write the file atomically, then ask the running daemon to
/// re-read it (`POST /reload`); when nobody answers, restart the user service.
/// The window never talks to the real printer itself and never holds state the
/// file does not have.
fn run_window(locale: &'static str, hooks: Hooks) -> Result<u8, String> {
    adw::init().map_err(|e| {
        l10n::tr(
            locale,
            "gui.err.no_display_window",
            &[("err", e.to_string())],
        )
    })?;

    let path = cfg_file::Config::path();
    let (config, file_error) = match cfg_file::Config::load(&path, locale) {
        Ok(Some(config)) => (config, None),
        // No file yet: the window starts empty and its first save creates one.
        Ok(None) => (cfg_file::Config::default(), None),
        // A file that does not parse is the user's data: show it, never
        // overwrite it until it is fixed.
        Err(e) => {
            eprintln!("{}: {e}", gui::NAME);
            (cfg_file::Config::default(), Some(e))
        }
    };

    let app = adw::Application::builder()
        .application_id(gui::APP_ID)
        .flags(adw::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        let labels = gui::window_labels(locale);
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(&labels.title)
            .default_width(640)
            .default_height(520)
            .build();

        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new(&labels.title, &labels.printers_title);
        header.set_title_widget(Some(&title));

        let add = icon_button("list-add-symbolic", &labels.add_tooltip);
        header.pack_end(&add);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);

        let overlay = adw::ToastOverlay::new();
        overlay.set_child(Some(&toolbar));
        window.set_content(Some(&overlay));

        let state = Rc::new(State {
            locale: Cell::new(locale),
            path: path.clone(),
            config: RefCell::new(config.clone()),
            file_error: file_error.clone(),
            probe_failed: Cell::new(false),
            window: window.clone(),
            toolbar: toolbar.clone(),
            overlay: overlay.clone(),
        });

        {
            let state = state.clone();
            add.connect_clicked(move |_| editor(&state, None));
        }

        rebuild(&state);
        window.present();

        // Printers configured in an earlier session may not be in CUPS yet
        // (the window used to only start the listener); register them so the
        // desktop's printer settings list them.
        register_queues(&state);

        if hooks.add {
            // The Xvfb smoke test also opens the "+" dialog, so the detection
            // list and the collapsed "Advanced" section are built for real.
            let state = state.clone();
            glib::timeout_add_local_once(Duration::from_millis(300), move || {
                editor(&state, None);
            });
        }
        if hooks.setup {
            // The smoke test presses "Set up" itself, so the first-run install
            // runs for real: a stub systemctl and a temporary HOME make it
            // write the unit without touching the machine running the test.
            let state = state.clone();
            glib::timeout_add_local_once(Duration::from_millis(300), move || {
                start_setup(&state);
            });
        }
        if hooks.uninstall {
            // And the other direction: uninstall must put the machine back into
            // the state the "Set up" row is for, so a test loop can repeat.
            let state = state.clone();
            glib::timeout_add_local_once(Duration::from_millis(300), move || {
                uninstall_service(&state);
            });
        }
        if hooks.window || hooks.add || hooks.setup || hooks.uninstall {
            // The Xvfb smoke test needs the real window to be built and shown,
            // then to go away without a user.
            let window = window.clone();
            let app = app.clone();
            let delay = if hooks.add {
                1200
            } else if hooks.setup {
                // Long enough for the install, and for the wait for a daemon
                // that the test machine does not run.
                8000
            } else {
                2000
            };
            glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                window.close();
                app.quit();
            });
        }
    });

    app.run_with_args::<&str>(&[]);
    Ok(gui::EXIT_ACCEPT)
}

/// Everything the callbacks need; one per window.
struct State {
    locale: Cell<&'static str>,
    path: PathBuf,
    config: RefCell<cfg_file::Config>,
    window: adw::ApplicationWindow,
    toolbar: adw::ToolbarView,
    overlay: adw::ToastOverlay,
    /// Set when the file exists but could not be read: the window shows the
    /// error and refuses to save until the file is fixed.
    file_error: Option<String>,
    /// Whether the last install could not reach the daemon. The row stays
    /// visible then, so a machine whose service "is running" but answers
    /// nothing can be set up again instead of looking finished.
    probe_failed: Cell<bool>,
}

/// Rebuild the page from the configuration in `state`. Cheap enough to do
/// after every change, and it keeps the widgets and the file from drifting.
fn rebuild(state: &Rc<State>) {
    let page = build_page(state);
    state.toolbar.set_content(Some(&page));
}

fn build_page(state: &Rc<State>) -> adw::PreferencesPage {
    let locale = state.locale.get();
    let labels = gui::window_labels(locale);
    let page = adw::PreferencesPage::new();

    // First run: there is no unit yet, or one that was never started. The row
    // is the whole difference between "the window only shows printers" and
    // "the window can set the machine up", so it comes before the printers.
    if setup_needed(state) {
        page.add(&setup_group(state, &labels));
    }
    page.add(&printers_group(state, &labels));
    page.add(&service_group(state, &labels));
    page.add(&diagnostics_group(state, &labels));
    page.add(&settings_group(state, &labels));
    page.add(&about_group(&labels));

    page
}

/// Whether the machine still needs the service installed or started.
fn setup_needed(state: &Rc<State>) -> bool {
    !service::installed() || !service::is_active() || state.probe_failed.get()
}

/// The first-run row: one button that installs (or starts) the service, so the
/// daemon runs without the terminal wizard.
fn setup_group(state: &Rc<State>, labels: &gui::WindowLabels) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title(&labels.setup_title);

    let row = adw::ActionRow::builder()
        .title(&labels.service_title)
        .subtitle(&labels.setup_body)
        .build();
    row.set_subtitle_lines(4);
    let button = gtk::Button::with_label(&labels.setup_button);
    button.add_css_class("suggested-action");
    button.set_valign(gtk::Align::Center);
    {
        let state = state.clone();
        button.connect_clicked(move |_| start_setup(&state));
    }
    row.add_suffix(&button);
    group.add(&row);
    group
}

/// Install (or just start) the user service, wait until the daemon answers,
/// then register the queues CUPS needs. With no printer configured there is
/// nothing to start — the daemon refuses to run without one — so this opens the
/// add dialog first, in the same order the terminal wizard uses.
fn start_setup(state: &Rc<State>) {
    let locale = state.locale.get();
    if state.config.borrow().printers.is_empty() {
        editor(state, None);
        return;
    }
    // A daemon that cannot even start is worse than one that is missing: check
    // the one program it refuses to run without before installing anything.
    if let Some((what, hint)) = missing_requirements().first() {
        let title = l10n::tr(
            locale,
            "gui.window.requirements_title",
            &[("what", what.to_string())],
        );
        let body = l10n::tr(
            locale,
            "gui.window.requirements_body",
            &[("what", what.to_string()), ("hint", hint.to_string())],
        );
        show_text(state, &title, &body);
        return;
    }
    match service::install(true) {
        Ok(_) => toast(state, &l10n::text(locale, "gui.window.setup_done")),
        Err(e) => {
            toast(
                state,
                &l10n::tr(locale, "gui.window.setup_failed", &[("err", e)]),
            );
            return;
        }
    }
    if wait_for_daemon(state, locale) {
        state.probe_failed.set(false);
        register_queues(state);
    } else {
        state.probe_failed.set(true);
        // A toast cannot hold `systemctl status` and a journal, and it cannot be
        // copied; the window this is reported to is the diagnostics dialog.
        let title = l10n::text(locale, "gui.window.not_answering_title");
        let body = diagnostics_text(state);
        show_text(state, &title, &body);
    }
    idle_rebuild(state);
}

/// The programs the daemon needs before it can do anything at all, with the one
/// line that installs each. Only `qpdf` is fatal at startup; without it the
/// service restarts forever and never listens.
fn missing_requirements() -> Vec<(&'static str, String)> {
    let mut missing = Vec::new();
    // Ask the way the unit will: a distro package puts qpdf in /usr/bin, while
    // on an atomic machine it usually comes from Homebrew, which a desktop
    // session's PATH does not necessarily contain.
    let found = service::find_program("qpdf")
        .map(|path| Command::new(path).arg("--version").output().is_ok())
        .unwrap_or(false);
    if !found {
        // rpm-ostree layers a package but needs a reboot, so Homebrew is the
        // first suggestion on Silverblue derivatives (Bluefin, Bazzite...).
        let hint = if service::atomic_host() {
            "brew install qpdf   ·   rpm-ostree install qpdf (needs a reboot)   ·   \
             or put a qpdf binary in ~/.local/bin"
        } else {
            "dnf install qpdf   ·   apt install qpdf   ·   pacman -S qpdf"
        };
        missing.push(("qpdf", hint.to_string()));
    }
    missing
}

/// The daemon binds its sockets right after `enable --now`, but systemd starts
/// it asynchronously: give it a few seconds before calling it unreachable.
fn wait_for_daemon(state: &Rc<State>, locale: &str) -> bool {
    let addrs: Vec<String> = state
        .config
        .borrow()
        .printers
        .iter()
        .map(|p| p.listen.clone())
        .collect();
    if addrs.is_empty() {
        return true;
    }
    for _ in 0..20 {
        if addrs
            .iter()
            .any(|addr| gui::http_get(addr, "/", locale).is_ok())
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// Everything needed to answer "why is it not printing", in one selectable
/// block: what the unit runs, what systemd thinks of it, the service journal,
/// and whether each port answers.
fn diagnostics_text(state: &Rc<State>) -> String {
    let locale = state.locale.get();
    let mut out = String::new();
    out.push_str(&format!("{} {}\n", gui::NAME, env!("CARGO_PKG_VERSION")));
    if let Some(image) = service::running_image() {
        out.push_str(&format!("AppImage: {}\n", image.display()));
        out.push_str(&format!(
            "installed copy: {}\n",
            service::installed_image().display()
        ));
    }
    let unit = service::packaged_unit().unwrap_or_else(service::user_unit_path);
    out.push_str(&format!("unit: {}\n", unit.display()));
    match std::fs::read_to_string(&unit) {
        Ok(text) => {
            for line in text.lines().filter(|line| {
                line.starts_with("ExecStart=") || line.starts_with("EnvironmentFile=")
            }) {
                out.push_str(line);
                out.push('\n');
            }
        }
        Err(e) => out.push_str(&format!("unit file: {e}\n")),
    }
    out.push_str("\n--- systemctl --user status ---\n");
    out.push_str(&service::status_text());
    out.push_str("\n--- journal ---\n");
    out.push_str(&service::journal_text(80));
    out.push_str("\n--- virtual printers ---\n");
    let printers: Vec<(String, String, String)> = state
        .config
        .borrow()
        .printers
        .iter()
        .map(|printer| {
            (
                printer.name.clone(),
                printer.listen.clone(),
                printer.real.clone(),
            )
        })
        .collect();
    let ipp_usb = usb::installed();
    for (name, listen, real) in printers {
        let line = match gui::http_get(&listen, "/", locale) {
            Ok(_) => l10n::tr(
                locale,
                "gui.window.diag_answers",
                &[("listen", listen.clone())],
            ),
            Err(e) => l10n::tr(
                locale,
                "gui.window.unreachable",
                &[("addr", listen.clone()), ("err", e)],
            ),
        };
        out.push_str(&format!("{name}: {line}\n"));
        // A USB printer is only an IPP printer through ipp-usb; say so here,
        // next to the port probe that is already asking why nothing answers.
        if let Some(uri) = usb::missing([real.as_str()], ipp_usb) {
            out.push_str(&format!(
                "  {}\n",
                l10n::tr(
                    locale,
                    "common.ipp_usb_missing",
                    &[("uri", uri.to_string())]
                )
            ));
        }
    }
    out
}

/// A dialog for text a person may need to read twice, select and paste
/// somewhere else: wrapped, scrollable, selectable, with a Copy button. Toasts
/// are for "saved", not for a journal.
fn show_text(state: &Rc<State>, title: &str, body: &str) {
    let locale = state.locale.get();
    let labels = gui::window_labels(locale);

    let label = gtk::Label::new(Some(body));
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_selectable(true);
    label.set_xalign(0.0);
    label.set_valign(gtk::Align::Start);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&label));
    scroller.set_min_content_height(340);
    scroller.set_min_content_width(620);
    scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);

    let body_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body_box.append(&scroller);
    let copy = gtk::Button::with_label(&labels.copy);
    copy.add_css_class("flat");
    copy.set_halign(gtk::Align::End);
    {
        let state = state.clone();
        let text = body.to_string();
        let copied = labels.copied.clone();
        copy.connect_clicked(move |_| {
            // RootExt::display and WidgetExt::display both apply to a window, so
            // name the one that returns the GdkDisplay this clipboard lives on.
            let display = gtk::prelude::WidgetExt::display(&state.window);
            display.clipboard().set_text(&text);
            toast(&state, &copied);
        });
    }
    body_box.append(&copy);

    let dialog = adw::AlertDialog::builder()
        .heading(title)
        .extra_child(&body_box)
        .build();
    dialog.add_response("close", &labels.close);
    dialog.set_close_response("close");
    dialog.present(Some(&state.window));
}

fn diagnostics_group(state: &Rc<State>, labels: &gui::WindowLabels) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title(&labels.diagnostics_title);

    let row = adw::ActionRow::builder()
        .title(&labels.status_title)
        .subtitle(&labels.diagnostics_hint)
        .build();
    row.set_subtitle_lines(2);
    let open = gtk::Button::with_label(&labels.open);
    open.add_css_class("flat");
    {
        let state = state.clone();
        open.connect_clicked(move |_| {
            let body = diagnostics_text(&state);
            let locale = state.locale.get();
            let title = l10n::text(locale, "gui.window.diagnostics_title");
            show_text(&state, &title, &body);
        });
    }
    row.add_suffix(&open);
    group.add(&row);
    group
}

fn printers_group(state: &Rc<State>, labels: &gui::WindowLabels) -> adw::PreferencesGroup {
    let locale = state.locale.get();
    let group = adw::PreferencesGroup::new();
    group.set_title(&labels.printers_title);

    let printers: Vec<(String, String, String)> = state
        .config
        .borrow()
        .printers
        .iter()
        .map(|p| (p.name.clone(), p.listen.clone(), p.real.clone()))
        .collect();

    if printers.is_empty() {
        let row = adw::ActionRow::builder()
            .title(&labels.no_printers)
            .subtitle(&labels.no_printers_body)
            .build();
        row.set_subtitle_lines(4);
        group.add(&row);
    }

    for (index, (name, listen, real)) in printers.iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(name)
            .subtitle(gui::row_detail(locale, listen, real))
            .build();

        let flip_button = icon_button("media-playback-start-symbolic", &labels.flip_tooltip);
        let status = icon_button("zoom-in-symbolic", &labels.status_title);
        let edit = icon_button("document-edit-symbolic", &labels.edit_tooltip);
        let remove = icon_button("user-trash-symbolic", &labels.remove_tooltip);

        {
            let state = state.clone();
            let addr = listen.clone();
            flip_button.connect_clicked(move |_| flip(&state, &addr));
        }
        {
            let state = state.clone();
            let addr = listen.clone();
            status.connect_clicked(move |_| show_status(&state, &addr));
        }
        {
            let state = state.clone();
            edit.connect_clicked(move |_| editor(&state, Some(index)));
        }
        {
            let state = state.clone();
            remove.connect_clicked(move |_| confirm_remove(&state, index));
        }

        row.add_suffix(&flip_button);
        row.add_suffix(&status);
        row.add_suffix(&edit);
        row.add_suffix(&remove);
        group.add(&row);
    }

    group
}

fn service_group(state: &Rc<State>, labels: &gui::WindowLabels) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title(&labels.service_title);

    let detail = if !service::installed() {
        labels.service_not_installed.clone()
    } else if service::available() {
        service::state_text()
    } else {
        labels.service_missing.clone()
    };
    let row = adw::ActionRow::builder()
        .title(service::UNIT)
        .subtitle(&detail)
        .build();
    let restart = gtk::Button::with_label(&labels.restart);
    restart.add_css_class("flat");
    {
        let state = state.clone();
        restart.connect_clicked(move |_| restart_service(&state));
    }
    row.add_suffix(&restart);
    // The other half of "Set up": with no unit left, the row comes back and the
    // machine can be set up again — which is also how the first run is tested on
    // a machine that already has the service.
    if service::installed() {
        let uninstall = gtk::Button::with_label(&labels.uninstall);
        uninstall.add_css_class("flat");
        uninstall.add_css_class("destructive-action");
        {
            let state = state.clone();
            uninstall.connect_clicked(move |_| confirm_uninstall(&state));
        }
        row.add_suffix(&uninstall);
    }
    group.add(&row);

    group
}

/// Ask before removing a unit: the printers stay, but a service that vanishes
/// under a running print job is still worth a confirmation.
fn confirm_uninstall(state: &Rc<State>) {
    let locale = state.locale.get();
    let labels = gui::window_labels(locale);
    let dialog = adw::AlertDialog::builder()
        .heading(&labels.uninstall_title)
        .body(&labels.uninstall_body)
        .build();
    dialog.add_response("cancel", &labels.cancel);
    dialog.add_response("uninstall", &labels.uninstall);
    dialog.set_response_appearance("uninstall", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");
    {
        let state = state.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "uninstall" {
                uninstall_service(&state);
            }
        });
    }
    dialog.present(Some(&state.window));
}

fn uninstall_service(state: &Rc<State>) {
    let locale = state.locale.get();
    state.probe_failed.set(false);
    match service::uninstall() {
        Ok(done) => {
            let key = if done.packaged {
                "gui.window.uninstalled_packaged"
            } else {
                "gui.window.uninstalled"
            };
            toast(state, &l10n::text(locale, key));
        }
        Err(e) => toast(
            state,
            &l10n::tr(locale, "gui.window.uninstall_failed", &[("err", e)]),
        ),
    }
    idle_rebuild(state);
}

fn settings_group(state: &Rc<State>, labels: &gui::WindowLabels) -> adw::PreferencesGroup {
    let locale = state.locale.get();
    let group = adw::PreferencesGroup::new();
    group.set_title(&labels.settings_title);

    let codes = l10n::codes();
    let combo = adw::ComboRow::builder()
        .title(&labels.language_title)
        .build();
    let model = gtk::StringList::new(&[]);
    let mut selected = 0u32;
    for (index, code) in codes.iter().enumerate() {
        let native = l10n::native_name(code).unwrap_or(code);
        model.append(&format!("{code} ({native})"));
        if *code == locale {
            selected = index as u32;
        }
    }
    combo.set_model(Some(&model));
    combo.set_selected(selected);
    {
        let state = state.clone();
        combo.connect_selected_notify(move |row| {
            if let Some(code) = codes.get(row.selected() as usize) {
                set_language(&state, code);
            }
        });
    }
    group.add(&combo);

    let path = adw::ActionRow::builder()
        .title(&labels.config_title)
        .subtitle(state.path.display().to_string())
        .build();
    group.add(&path);

    group
}

fn about_group(labels: &gui::WindowLabels) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title(&labels.about_group);

    let version = adw::ActionRow::builder()
        .title(&labels.version_title)
        .subtitle(env!("CARGO_PKG_VERSION"))
        .build();
    group.add(&version);

    let project = adw::ActionRow::builder()
        .title(gui::NAME)
        .subtitle("https://github.com/Wraindrock/ipp-duplexd-fork")
        .build();
    group.add(&project);

    group
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class("flat");
    button.set_valign(gtk::Align::Center);
    button.set_tooltip_text(Some(tooltip));
    button
}

fn toast(state: &Rc<State>, text: &str) {
    state.overlay.add_toast(adw::Toast::new(text));
}

fn idle_rebuild(state: &Rc<State>) {
    let state = state.clone();
    glib::idle_add_local_once(move || rebuild(&state));
}

/// Ask the daemon behind this address to print the backs now.
fn flip(state: &Rc<State>, addr: &str) {
    let locale = state.locale.get();
    match gui::http_get(addr, "/flip", locale) {
        Ok(body) => toast(state, body.trim()),
        Err(e) => toast(state, &gui::unreachable(locale, addr, &e)),
    }
}

/// Show the daemon's own status page (already in the window's language).
fn show_status(state: &Rc<State>, addr: &str) {
    let locale = state.locale.get();
    let labels = gui::window_labels(locale);
    let body = match gui::http_get(addr, "/", locale) {
        Ok(body) => body,
        Err(e) => gui::unreachable(locale, addr, &e),
    };
    let label = gtk::Label::new(Some(body.trim()));
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.set_valign(gtk::Align::Start);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&label));
    scroller.set_min_content_height(180);
    scroller.set_min_content_width(420);

    let dialog = adw::AlertDialog::builder()
        .heading(&labels.status_title)
        .extra_child(&scroller)
        .build();
    dialog.add_response("close", &labels.cancel);
    dialog.set_close_response("close");
    dialog.present(Some(&state.window));
}

fn restart_service(state: &Rc<State>) {
    let locale = state.locale.get();
    match service::restart() {
        Ok(()) => {
            toast(state, &l10n::text(locale, "gui.window.restarted"));
            idle_rebuild(state);
        }
        Err(e) => toast(
            state,
            &l10n::tr(locale, "gui.window.restart_failed", &[("err", e)]),
        ),
    }
}

/// Put every printer the daemon answers for into CUPS as a driverless queue,
/// which is what the desktop's printer settings list. A printer whose daemon
/// does not answer is skipped: `lpadmin -m everywhere` has to ask it.
fn register_queues(state: &Rc<State>) {
    let locale = state.locale.get();
    let printers = gui::queues_to_register(&state.config.borrow().printers, |addr| {
        gui::http_get(addr, "/", locale).is_ok()
    });
    let mut failures = Vec::new();
    for printer in printers {
        if let Err(e) = queues::ensure(&printer.name, &printer.listen) {
            failures.push(l10n::tr(
                locale,
                "gui.window.queue_failed",
                &[("name", printer.name.clone()), ("err", e)],
            ));
        }
    }
    if !failures.is_empty() {
        toast(state, &failures.join("; "));
    }
}

fn set_language(state: &Rc<State>, code: &'static str) {
    if state.locale.get() == code {
        return;
    }
    state.config.borrow_mut().settings.language = Some(code.to_string());
    state.locale.set(code);
    l10n::apply(code);
    commit(state);
}

/// Write the file, then make the running daemon use it: reload the printers
/// that answer, restart the service when none does.
fn commit(state: &Rc<State>) {
    let locale = state.locale.get();
    if let Some(error) = &state.file_error {
        toast(state, error);
        return;
    }
    let config = state.config.borrow().clone();
    if let Err(e) = config.validate(locale) {
        toast(
            state,
            &l10n::tr(locale, "gui.window.save_failed", &[("err", e)]),
        );
        return;
    }
    if let Err(e) = config.save(&state.path, locale) {
        toast(
            state,
            &l10n::tr(locale, "gui.window.save_failed", &[("err", e)]),
        );
        return;
    }
    let addrs: Vec<String> = config
        .printers
        .iter()
        .map(|printer| printer.listen.clone())
        .collect();
    if addrs.is_empty() {
        // The last printer is gone: nothing to reload, nothing to register and
        // nothing to install a service for. Saving is the whole operation.
        toast(state, &l10n::text(locale, "gui.window.saved"));
    } else if gui::reload_everywhere(&addrs, locale) == 0 {
        // Nothing answers: either the unit is there but stopped, or this is
        // the very first save and there is no unit at all. The second case is
        // the one the terminal wizard used to be needed for.
        if service::installed() {
            match service::restart() {
                Ok(()) => toast(state, &l10n::text(locale, "gui.window.restarted")),
                Err(e) => toast(
                    state,
                    &l10n::tr(locale, "gui.window.restart_failed", &[("err", e)]),
                ),
            }
        } else {
            start_setup(state);
            return;
        }
    } else {
        toast(state, &l10n::text(locale, "gui.window.saved"));
        // The daemon has the printers now; CUPS needs a queue per printer or
        // the desktop's printer settings stay empty.
        register_queues(state);
    }
    idle_rebuild(state);
}

/// Add (`None`) or edit (`Some(index)`) one virtual printer.
fn editor(state: &Rc<State>, index: Option<usize>) {
    let locale = state.locale.get();
    let labels = gui::window_labels(locale);

    let name = adw::EntryRow::new();
    name.set_title(&labels.field_name);
    let listen = adw::EntryRow::new();
    listen.set_title(&labels.field_listen);
    let real = adw::EntryRow::new();
    real.set_title(&labels.field_real);

    let used: Vec<String> = state
        .config
        .borrow()
        .printers
        .iter()
        .map(|printer| printer.listen.clone())
        .collect();
    listen.set_text(&gui::default_listen(&used));

    let heading = match index {
        Some(index) => {
            let config = state.config.borrow();
            let printer = &config.printers[index];
            name.set_text(&printer.name);
            listen.set_text(&printer.listen);
            real.set_text(&printer.real);
            l10n::tr(
                locale,
                "gui.window.dialog_edit",
                &[("name", printer.name.clone())],
            )
        }
        None => labels.dialog_add.clone(),
    };

    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let fields = gtk::ListBox::new();
    fields.add_css_class("boxed-list");
    fields.append(&name);
    fields.append(&listen);
    fields.append(&real);

    let dialog = adw::AlertDialog::builder()
        .heading(&heading)
        .extra_child(&body)
        .build();

    match index {
        // Editing an existing printer: the fields are the point.
        Some(_) => body.append(&fields),
        // Adding: offer what CUPS already knows, and keep the manual fields
        // folded away unless there was nothing to detect.
        None => {
            let (detected, failure) = match discover::cups_printers() {
                Ok(printers) => (printers, None),
                Err(err) => (Vec::new(), Some(err)),
            };
            let nothing_found = detected.is_empty();
            let group = adw::PreferencesGroup::builder()
                .title(&labels.detected_title)
                .build();
            if nothing_found {
                let row = adw::ActionRow::builder()
                    .title(&labels.detected_none)
                    .build();
                if let Some(err) = failure {
                    row.set_subtitle(&l10n::tr(
                        locale,
                        "gui.window.detected_failed",
                        &[("err", err)],
                    ));
                }
                group.add(&row);
            } else {
                group.set_description(Some(&labels.detected_hint));
                for entry in detected {
                    let row = adw::ActionRow::builder()
                        .title(&entry.queue)
                        .subtitle(&entry.model)
                        .activatable(true)
                        .build();
                    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
                    let state = state.clone();
                    let dialog = dialog.clone();
                    row.connect_activated(move |_| {
                        add_detected(&state, &entry);
                        dialog.close();
                    });
                    group.add(&row);
                }
            }
            body.append(&group);

            let advanced = adw::ExpanderRow::builder()
                .title(&labels.advanced)
                .subtitle(&labels.advanced_hint)
                .build();
            advanced.set_expanded(nothing_found);
            advanced.add_row(&name);
            advanced.add_row(&listen);
            advanced.add_row(&real);
            body.append(&advanced);
        }
    }

    dialog.add_response("cancel", &labels.cancel);
    dialog.add_response("save", &labels.save);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");

    {
        let state = state.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "save" {
                return;
            }
            let mut candidate = state.config.borrow().clone();
            let mut printer = match index {
                Some(index) => candidate.printers[index].clone(),
                None => cfg_file::Printer::new("", "", ""),
            };
            printer.name = name.text().to_string();
            printer.listen = listen.text().to_string();
            printer.real = real.text().to_string();
            match index {
                Some(index) => candidate.printers[index] = printer,
                None => candidate.printers.push(printer),
            }
            if let Err(e) = candidate.validate(locale) {
                toast(
                    &state,
                    &l10n::tr(locale, "gui.window.save_failed", &[("err", e)]),
                );
                return;
            }
            state.config.replace(candidate);
            commit(&state);
        });
    }

    dialog.present(Some(&state.window));
}

/// Add one detected queue: a unique name, the next free address, and the queue
/// as the printer to forward to.
fn add_detected(state: &Rc<State>, detected: &discover::Discovered) {
    let locale = state.locale.get();
    let mut candidate = state.config.borrow().clone();
    let names: Vec<String> = candidate
        .printers
        .iter()
        .map(|printer| printer.name.clone())
        .collect();
    let listens: Vec<String> = candidate
        .printers
        .iter()
        .map(|printer| printer.listen.clone())
        .collect();
    candidate
        .printers
        .push(gui::printer_from_detected(detected, &names, &listens));
    if let Err(e) = candidate.validate(locale) {
        toast(
            state,
            &l10n::tr(locale, "gui.window.save_failed", &[("err", e)]),
        );
        return;
    }
    state.config.replace(candidate);
    commit(state);
}
fn confirm_remove(state: &Rc<State>, index: usize) {
    let locale = state.locale.get();
    let labels = gui::window_labels(locale);
    let name = state
        .config
        .borrow()
        .printers
        .get(index)
        .map(|printer| printer.name.clone())
        .unwrap_or_default();
    let queue_name = name.clone();

    let dialog = adw::AlertDialog::builder()
        .heading(l10n::tr(
            locale,
            "gui.window.remove_title",
            &[("name", name)],
        ))
        .body(&labels.remove_body)
        .build();
    dialog.add_response("cancel", &labels.cancel);
    dialog.add_response("remove", &labels.remove);
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");

    {
        let state = state.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "remove" {
                return;
            }
            let mut candidate = state.config.borrow().clone();
            if index < candidate.printers.len() {
                candidate.printers.remove(index);
            }
            state.config.replace(candidate);
            commit(&state);
            // The queue is what the desktop lists; leaving it behind would
            // keep a dead printer in the settings.
            if let Err(e) = queues::remove(&queue_name) {
                toast(
                    &state,
                    &l10n::tr(
                        locale,
                        "gui.window.queue_removed_failed",
                        &[("name", queue_name.clone()), ("err", e)],
                    ),
                );
            }
        });
    }

    dialog.present(Some(&state.window));
}
