//! The terminal front end: the same tree the window shows, for machines with
//! no desktop (and for scripts, via `--yes`).
//!
//! It writes the same `config.toml` the window writes, through the same
//! `config` / `queues` / `service` code, so the two front ends cannot drift
//! apart; this file only owns the questions and the printing.

use ipp_duplexd_fork::config::{Config, Printer};
use ipp_duplexd_fork::discover;
use ipp_duplexd_fork::gui;
use ipp_duplexd_fork::hplip;
use ipp_duplexd_fork::l10n;
use ipp_duplexd_fork::queues;
use ipp_duplexd_fork::service;
use ipp_duplexd_fork::usb;
use rust_i18n::t;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

rust_i18n::i18n!("locales", fallback = "en");

static ASSUME_DEFAULTS: AtomicBool = AtomicBool::new(false);
static LANG: OnceLock<&'static str> = OnceLock::new();

fn lang() -> &'static str {
    LANG.get().copied().unwrap_or(l10n::FALLBACK)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match arg_value(&argv, "--lang") {
        Some(tag) => match l10n::normalize(&tag) {
            Some(code) => {
                LANG.set(l10n::apply(code)).ok();
            }
            None => {
                eprintln!(
                    "{}\n{}",
                    t!(
                        "setup.err.bad_lang",
                        lang = tag,
                        list = l10n::describe_all()
                    ),
                    t!("setup.usage")
                );
                std::process::exit(2);
            }
        },
        None => {
            let code = l10n::from_env().unwrap_or(l10n::FALLBACK);
            LANG.set(l10n::apply(code)).ok();
        }
    }
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        print!("{}", t!("setup.usage"));
        return;
    }
    let mut printer_uri = None;
    let mut listen = None;
    let mut do_service = true;
    let mut do_queue = true;
    let mut args = argv.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--printer" => printer_uri = args.next().cloned(),
            "--listen" => listen = args.next().cloned(),
            "--no-service" => do_service = false,
            "--no-queue" => do_queue = false,
            "--yes" => ASSUME_DEFAULTS.store(true, Ordering::Relaxed),
            "--lang" => {
                args.next();
            }
            other => {
                eprintln!(
                    "{}\n\n{}",
                    t!("setup.err.unknown_option", arg = other),
                    t!("setup.usage")
                );
                std::process::exit(2);
            }
        }
    }
    let mut config = match Config::load(&Config::path(), lang()) {
        Ok(Some(config)) => config,
        Ok(None) => Config::default(),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    if printer_uri.is_some() || ASSUME_DEFAULTS.load(Ordering::Relaxed) {
        // The scripted path: one printer, defaults everywhere else.
        if let Err(e) = quick_setup(&mut config, printer_uri, listen, do_service, do_queue) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    refresh_driverless_ports(&config);
    menu(&mut config);
}

/// ipp-usb allocates its port from 60000 upwards; a driverless queue that
/// pointed at an older one stops printing after a restart. Same fix as the
/// window's, so the two front ends behave alike.
fn refresh_driverless_ports(config: &Config) {
    let Some(port) = usb::serving_port() else {
        return;
    };
    let known = discover::cups_printers().unwrap_or_default();
    for printer in &config.printers {
        let Some(queue) = hplip::queue_of_real(&printer.real) else {
            continue;
        };
        let Some(device) = known
            .iter()
            .find(|known| known.queue == queue)
            .and_then(|known| known.device_uri.as_deref())
        else {
            continue;
        };
        let Some(pointed_at) = usb::ipp_usb_port_of(device) else {
            continue;
        };
        if pointed_at == port {
            continue;
        }
        match queues::repoint(&queue, &hplip::ipp_usb_uri(port)) {
            Ok(()) => println!("{}", t!("gui.window.port_moved", port = port.to_string())),
            Err(e) => println!(
                "{}",
                l10n::tr(
                    lang(),
                    "gui.window.queue_failed",
                    &[("name", queue), ("err", e)]
                )
            ),
        }
    }
}

/// Non-interactive setup, for `--yes` and `--printer`.
fn quick_setup(
    config: &mut Config,
    printer_uri: Option<String>,
    listen: Option<String>,
    do_service: bool,
    do_queue: bool,
) -> Result<(), String> {
    let detected = discover::cups_printers().unwrap_or_default();
    let real = match printer_uri {
        Some(uri) => uri,
        None => detected
            .first()
            .map(|printer| printer.uri.clone())
            .ok_or_else(|| t!("setup.err.no_printer").to_string())?,
    };
    let listen = listen.unwrap_or_else(|| next_free_listen(config));
    let name = suggested_name(config, &detected, &real);
    let printer = Printer::new(&name, &listen, &real);
    config.printers.push(printer);
    config.validate(lang())?;
    config.save(&Config::path(), lang())?;
    println!("{}", t!("gui.window.saved"));
    if do_service {
        start_service()?;
    }
    if do_queue {
        register_queues(config, false);
    }
    Ok(())
}

fn menu(config: &mut Config) {
    loop {
        println!();
        println!("{}", t!("tui.menu.title"));
        println!("  {}", t!("tui.menu.printers"));
        println!("  {}", t!("tui.menu.jobs"));
        println!("  {}", t!("tui.menu.service"));
        println!("  {}", t!("tui.menu.diagnostics"));
        println!("  {}", t!("tui.menu.settings"));
        println!("  {}", t!("tui.menu.quit"));
        match ask(&t!("tui.prompt"), "") {
            c if c == "1" => printers_menu(config),
            c if c == "2" => jobs_menu(config),
            c if c == "3" => service_menu(),
            c if c == "4" => {
                print!(
                    "{}",
                    ipp_duplexd_fork::diagnostics::text(&config.printers, lang())
                );
            }
            c if c == "5" => settings_menu(config),
            c if c == "q" || c == "Q" || c.is_empty() => {
                println!("{}", t!("tui.bye"));
                return;
            }
            _ => println!("{}", t!("tui.invalid")),
        }
    }
}

// ------------------------------------------------------------------ printers

fn printers_menu(config: &mut Config) {
    loop {
        println!();
        println!("{}", t!("gui.window.printers_title"));
        if config.printers.is_empty() {
            println!("  {}", t!("gui.window.no_printers"));
        }
        let known = discover::cups_printers().unwrap_or_default();
        for (i, printer) in config.printers.iter().enumerate() {
            let state = match gui::printer_state_at(&printer.listen) {
                Some(3) => l10n::text(lang(), "gui.window.state_idle"),
                Some(4) => l10n::text(lang(), "gui.window.state_busy"),
                Some(_) => l10n::text(lang(), "gui.window.state_stopped"),
                None => l10n::text(lang(), "gui.window.state_unknown"),
            };
            let driver = match hplip::driver(&printer.real, &known) {
                Some(hplip::Driver::Ipp) => l10n::text(lang(), "gui.window.driver_ipp"),
                Some(hplip::Driver::Hplip) => l10n::text(lang(), "gui.window.driver_hplip"),
                None => String::new(),
            };
            println!(
                "  {}. {}  {} -> {}  · {state} · {driver}",
                i + 1,
                printer.name,
                printer.listen,
                printer.real
            );
            // The same warnings the window puts under the row.
            let queue = hplip::queue_and_device(&printer.real, &known);
            let absent = queue
                .as_ref()
                .and_then(|(_, device)| hplip::serial_of(device))
                .and_then(usb::attached)
                == Some(false);
            if absent {
                println!("     {}", l10n::text(lang(), "gui.window.usb_absent"));
            } else if let Some((queue_name, _)) = queue {
                let serving = usb::serving();
                if !serving && usb::service_active() {
                    println!(
                        "     {}",
                        l10n::tr(
                            lang(),
                            "gui.window.ipp_usb_hint",
                            &[("queue", queue_name.clone())]
                        )
                    );
                } else if serving {
                    println!(
                        "     {}",
                        l10n::tr(
                            lang(),
                            "gui.window.hplip_hint",
                            &[("queue", queue_name.clone())]
                        )
                    );
                }
            }
        }
        println!("  {}", t!("tui.printers.actions"));
        let choice = ask(&t!("tui.prompt"), "");
        match choice.as_str() {
            "a" => add_printer(config),
            "e" => {
                if let Some(index) = pick_printer(config) {
                    edit_printer(config, index);
                }
            }
            "d" => {
                if let Some(index) = pick_printer(config) {
                    remove_printer(config, index);
                }
            }
            "f" => {
                if let Some(index) = pick_printer(config) {
                    flip(config, index);
                }
            }
            "s" => {
                if let Some(index) = pick_printer(config) {
                    status(config, index);
                }
            }
            "r" => {
                if let Some(index) = pick_printer(config) {
                    switch_driver(config, index);
                }
            }
            "g" => register_queues(config, false),
            "b" | "q" | "" => return,
            _ => println!("{}", t!("tui.invalid")),
        }
    }
}

fn pick_printer(config: &Config) -> Option<usize> {
    if config.printers.is_empty() {
        return None;
    }
    let answer = ask(&t!("tui.printers.which"), "1");
    answer
        .parse::<usize>()
        .ok()
        .filter(|n| *n >= 1 && *n <= config.printers.len())
        .map(|n| n - 1)
}

fn add_printer(config: &mut Config) {
    let detected = discover::cups_printers().unwrap_or_default();
    if !detected.is_empty() {
        println!("{}", t!("gui.window.detected_title"));
        for (i, printer) in detected.iter().enumerate() {
            println!("  {}. {} ({})", i + 1, printer.queue, printer.model);
        }
    }
    let real = ask(
        &t!("gui.window.field_real"),
        detected.first().map(|p| p.uri.as_str()).unwrap_or(""),
    );
    let listen = ask(&t!("gui.window.field_listen"), &next_free_listen(config));
    let name = ask(
        &t!("gui.window.field_name"),
        &suggested_name(config, &detected, &real),
    );
    let mut printer = Printer::new(&name, &listen, &real);
    if confirm(&t!("setup.ask.advanced"), false) {
        printer.blank = ask(&t!("gui.window.field_blank"), &printer.blank);
        printer.settle = ask_number(&t!("gui.window.field_settle"), printer.settle);
        printer.poll = ask_number(&t!("gui.window.field_poll"), printer.poll);
        printer.rotate_even = if confirm(&t!("gui.window.field_rotate"), printer.rotate_even == 180)
        {
            180
        } else {
            0
        };
        printer.auto_continue =
            ask_number(&t!("gui.window.field_auto_continue"), printer.auto_continue);
        printer.gui = confirm(&t!("gui.window.field_gui"), printer.gui);
    }
    config.printers.push(printer);
    save_and_register(config, false);
}

fn edit_printer(config: &mut Config, index: usize) {
    let mut printer = config.printers[index].clone();
    let previous = printer.name.clone();
    printer.name = ask(&t!("gui.window.field_name"), &printer.name);
    printer.listen = ask(&t!("gui.window.field_listen"), &printer.listen);
    printer.real = ask(&t!("gui.window.field_real"), &printer.real);
    printer.note = ask(&t!("gui.window.field_note"), &printer.note);
    printer.blank = ask(&t!("gui.window.field_blank"), &printer.blank);
    printer.settle = ask_number(&t!("gui.window.field_settle"), printer.settle);
    printer.poll = ask_number(&t!("gui.window.field_poll"), printer.poll);
    printer.rotate_even = if confirm(&t!("gui.window.field_rotate"), printer.rotate_even == 180) {
        180
    } else {
        0
    };
    printer.auto_continue =
        ask_number(&t!("gui.window.field_auto_continue"), printer.auto_continue);
    printer.gui = confirm(&t!("gui.window.field_gui"), printer.gui);
    config.printers[index] = printer.clone();
    save_and_register(config, false);
    if previous != printer.name {
        let _ = queues::remove(&previous);
    }
}

fn remove_printer(config: &mut Config, index: usize) {
    let printer = config.printers.remove(index);
    if confirm(&t!("gui.window.remove_body"), true) {
        let _ = queues::remove(&printer.name);
    }
    save_and_register(config, false);
}

fn flip(config: &Config, index: usize) {
    let printer = &config.printers[index];
    match gui::http_get(&printer.listen, "/flip", lang()) {
        Ok(text) => println!("{text}"),
        Err(e) => println!(
            "{}",
            l10n::tr(
                lang(),
                "gui.window.unreachable",
                &[("addr", printer.listen.clone()), ("err", e)]
            )
        ),
    }
}

fn status(config: &Config, index: usize) {
    let printer = &config.printers[index];
    match gui::http_get(&printer.listen, "/", lang()) {
        Ok(text) => println!("{text}"),
        Err(e) => println!(
            "{}",
            l10n::tr(
                lang(),
                "gui.window.unreachable",
                &[("addr", printer.listen.clone()), ("err", e)]
            )
        ),
    }
}

fn switch_driver(config: &Config, index: usize) {
    let printer = &config.printers[index];
    let known = discover::cups_printers().unwrap_or_default();
    let dir = service::data_dir();
    // One action: put the queue on the other driver. Which way that is follows
    // from the driver in use -- same rule as the window's single button.
    if let Some((queue, device)) = hplip::queue_and_device(&printer.real, &known) {
        let Some(port) = usb::serving_port() else {
            println!("{}", t!("gui.window.ipp_usb_still_down"));
            return;
        };
        if !confirm(
            &t!("gui.window.ipp_recover_title", queue = queue.clone()),
            true,
        ) {
            return;
        }
        match queues::ppd_text(&queue).and_then(|ppd| {
            queues::switch_to_ipp_usb(&queue, &device, &ppd, &hplip::ipp_usb_uri(port), &dir)
        }) {
            Ok(()) => println!("{}", t!("gui.window.hplip_done", queue = queue)),
            Err(e) => println!("{}", t!("gui.window.hplip_failed", queue = queue, err = e)),
        }
        return;
    }
    if let Some(queue) = hplip::queue_of_real(&printer.real) {
        if queues::has_backup(&dir, &queue) {
            match queues::restore(&queue, &dir) {
                Ok(device) => println!(
                    "{}",
                    l10n::tr(
                        lang(),
                        "gui.window.hplip_restored",
                        &[("queue", queue), ("device", device)]
                    )
                ),
                Err(e) => println!(
                    "{}",
                    l10n::tr(
                        lang(),
                        "gui.window.hplip_restore_failed",
                        &[("queue", queue), ("err", e)]
                    )
                ),
            }
            return;
        }
    }
    println!("{}", t!("tui.driver.none"));
}

// ---------------------------------------------------------------------- jobs

fn jobs_menu(config: &Config) {
    loop {
        println!();
        println!("{}", t!("gui.window.jobs_title"));
        let known = discover::cups_printers().unwrap_or_default();
        let mut listed = 0;
        for printer in &config.printers {
            match gui::jobs_at(&printer.listen) {
                Ok(jobs) if jobs.is_empty() => {
                    println!(
                        "  {}",
                        l10n::tr(
                            lang(),
                            "gui.window.jobs_none",
                            &[("name", printer.name.clone())]
                        )
                    );
                }
                Ok(jobs) => {
                    for job in jobs {
                        listed += 1;
                        println!(
                            "  {}. [{}] #{} {} — {} {}",
                            listed, printer.name, job.id, job.name, job.user, job.message
                        );
                    }
                }
                Err(e) => println!(
                    "  {}",
                    l10n::tr(
                        lang(),
                        "gui.window.jobs_failed",
                        &[("name", printer.name.clone()), ("err", e)]
                    )
                ),
            }
        }
        let _ = known;
        println!("  {}", t!("tui.jobs.actions"));
        let choice = ask(&t!("tui.prompt"), "");
        match choice.as_str() {
            "r" => continue,
            "c" => {
                let name = ask(&t!("tui.jobs.printer"), "");
                let id = ask(&t!("tui.jobs.id"), "");
                let (Some(printer), Ok(id)) = (
                    config.printers.iter().find(|p| p.name == name),
                    id.trim().parse::<i32>(),
                ) else {
                    println!("{}", t!("tui.invalid"));
                    continue;
                };
                match gui::cancel_job_at(&printer.listen, id) {
                    Ok(()) => println!("{}", t!("gui.window.job_cancelled", id = id.to_string())),
                    Err(e) => println!(
                        "{}",
                        t!("gui.window.job_cancel_failed", id = id.to_string(), err = e)
                    ),
                }
            }
            "b" | "q" | "" => return,
            _ => println!("{}", t!("tui.invalid")),
        }
    }
}

// ------------------------------------------------------------------- service

fn service_menu() {
    loop {
        println!();
        println!("{}", t!("gui.window.service_title"));
        let state = if service::installed() {
            if service::available() {
                service::state_text()
            } else {
                l10n::text(lang(), "gui.window.service_missing")
            }
        } else {
            l10n::text(lang(), "gui.window.service_not_installed")
        };
        println!("  {state}");
        let stale = service::stale_image().is_some();
        if stale {
            println!("  {}", t!("gui.window.service_stale_title"));
        }
        println!("  {}", t!("tui.service.actions"));
        match ask(&t!("tui.prompt"), "").as_str() {
            "i" => {
                if let Err(e) = start_service() {
                    println!("{e}");
                }
            }
            "u" => {
                if confirm(&t!("gui.window.uninstall_title"), false) {
                    match service::uninstall().map(|_| ()) {
                        Ok(()) => println!("{}", t!("gui.window.uninstalled")),
                        Err(e) => println!("{}", t!("gui.window.uninstall_failed", err = e)),
                    }
                }
            }
            "v" if stale => match service::refresh_image().and_then(|()| service::restart()) {
                Ok(()) => println!("{}", t!("gui.window.service_stale_done")),
                Err(e) => println!(
                    "{}",
                    l10n::tr(lang(), "gui.window.service_stale_failed", &[("err", e)])
                ),
            },
            "r" => match service::restart() {
                Ok(()) => println!("{}", t!("gui.window.restarted")),
                Err(e) => println!("{}", t!("gui.window.restart_failed", err = e)),
            },
            "b" | "q" | "" => return,
            _ => println!("{}", t!("tui.invalid")),
        }
    }
}

// ------------------------------------------------------------------ settings

fn settings_menu(config: &mut Config) {
    loop {
        println!();
        println!("{}", t!("gui.window.settings_title"));
        println!("  {}", t!("gui.window.config_title"));
        println!("    {}", Config::path().display());
        println!("  {}", t!("gui.window.language_title"));
        println!(
            "    {}",
            config.settings.language.as_deref().unwrap_or(lang())
        );
        println!("  {}", t!("tui.settings.actions"));
        match ask(&t!("tui.prompt"), "").as_str() {
            "l" => {
                let code = ask(&t!("gui.window.language_title"), lang());
                match l10n::normalize(&code) {
                    Some(code) => {
                        config.settings.language = Some(code.to_string());
                        save_and_register(config, true);
                    }
                    None => println!(
                        "{}",
                        t!(
                            "setup.err.bad_lang",
                            lang = code,
                            list = l10n::describe_all()
                        )
                    ),
                }
            }
            "b" | "q" | "" => return,
            _ => println!("{}", t!("tui.invalid")),
        }
    }
}

// -------------------------------------------------------------------- shared

fn save_and_register(config: &mut Config, quiet: bool) {
    if let Err(e) = config
        .validate(lang())
        .and_then(|_| config.save(&Config::path(), lang()))
    {
        println!(
            "{}",
            l10n::tr(lang(), "gui.window.save_failed", &[("err", e)])
        );
        return;
    }
    reload_daemons(config);
    register_queues(config, quiet);
}

fn reload_daemons(config: &Config) {
    let addrs: Vec<String> = config.printers.iter().map(|p| p.listen.clone()).collect();
    if gui::reload_everywhere(&addrs, lang()) > 0 {
        return;
    }
    // Nothing answers: either the unit is stopped or there is none yet.
    if service::installed() {
        let _ = service::restart();
    } else if let Err(e) = start_service() {
        println!("{e}");
    }
}

fn register_queues(config: &Config, quiet: bool) {
    for printer in &config.printers {
        if gui::http_get(&printer.listen, "/", lang()).is_err() {
            continue;
        }
        match queues::ensure(&printer.name, &printer.listen) {
            Ok(()) => {
                if !quiet {
                    println!("{}", t!("gui.window.queue_registered"));
                }
            }
            Err(e) => println!(
                "{}",
                l10n::tr(
                    lang(),
                    "gui.window.queue_failed",
                    &[("name", printer.name.clone()), ("err", e)]
                )
            ),
        }
    }
}

fn start_service() -> Result<(), String> {
    match service::install(false).map(|_| ()) {
        Ok(()) => {
            println!("{}", t!("gui.window.setup_done"));
            Ok(())
        }
        Err(e) => Err(l10n::tr(lang(), "gui.window.setup_failed", &[("err", e)])),
    }
}

fn next_free_listen(config: &Config) -> String {
    let used: Vec<String> = config.printers.iter().map(|p| p.listen.clone()).collect();
    gui::default_listen(&used)
}

fn suggested_name(config: &Config, detected: &[discover::Discovered], real: &str) -> String {
    let existing: Vec<String> = config.printers.iter().map(|p| p.name.clone()).collect();
    let queue = detected
        .iter()
        .find(|printer| printer.uri == real)
        .map(|printer| printer.queue.clone())
        .unwrap_or_else(|| "manual".to_string());
    discover::suggested_name(&queue, &existing)
}

// ------------------------------------------------------------------- prompting

fn ask(prompt: &str, default: &str) -> String {
    if ASSUME_DEFAULTS.load(Ordering::Relaxed) {
        return default.to_string();
    }
    print!("{prompt} [{default}]: ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return default.to_string();
    }
    let line = line.trim();
    if line.is_empty() {
        default.to_string()
    } else {
        line.to_string()
    }
}

fn ask_number(prompt: &str, default: u64) -> u64 {
    ask(prompt, &default.to_string()).parse().unwrap_or(default)
}

fn confirm(prompt: &str, default: bool) -> bool {
    let hint = if default { "Y/n" } else { "y/N" };
    if ASSUME_DEFAULTS.load(Ordering::Relaxed) {
        return default;
    }
    print!("{prompt} [{hint}]: ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return default;
    }
    match line.trim().to_ascii_lowercase().as_str() {
        "" => default,
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default,
    }
}

fn arg_value(argv: &[String], name: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == name)
        .and_then(|i| argv.get(i + 1))
        .cloned()
}
