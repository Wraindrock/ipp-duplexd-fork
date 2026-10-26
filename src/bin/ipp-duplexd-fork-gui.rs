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
use ipp_duplexd_fork::service;
use libadwaita as adw;
use rust_i18n::t;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
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
    let hook = std::env::var(gui::TEST_ENV).ok();
    let answer = gui::test_answer(hook.as_deref());
    let window_test = gui::window_test(hook.as_deref());
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
        Mode::Window => match run_window(args.lang, window_test) {
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
fn run_window(locale: &'static str, window_test: bool) -> Result<u8, String> {
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

        if window_test {
            // The Xvfb smoke test needs the real window to be built and shown,
            // then to go away without a user.
            let window = window.clone();
            let app = app.clone();
            glib::timeout_add_local_once(Duration::from_millis(400), move || {
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

    page.add(&printers_group(state, &labels));
    page.add(&service_group(state, &labels));
    page.add(&settings_group(state, &labels));
    page.add(&about_group(&labels));

    page
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

    let detail = if service::available() {
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
    group.add(&row);

    group
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
    if gui::reload_everywhere(&addrs, locale) == 0 {
        match service::restart() {
            Ok(()) => toast(state, &l10n::text(locale, "gui.window.restarted")),
            Err(e) => toast(
                state,
                &l10n::tr(locale, "gui.window.restart_failed", &[("err", e)]),
            ),
        }
    } else {
        toast(state, &l10n::text(locale, "gui.window.saved"));
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
        });
    }

    dialog.present(Some(&state.window));
}
