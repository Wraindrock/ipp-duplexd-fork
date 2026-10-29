//! Interactive setup for ipp-duplexd-fork: pick the real printer, write the config,
//! enable the systemd user service, and register the virtual printer as a
//! driverless CUPS queue.
//!
//! Every step can also be skipped or overridden with flags, and --yes runs
//! non-interactively with defaults.

use ipp_duplexd_fork::ipp::{self, Attr, Group, Msg};
use ipp_duplexd_fork::l10n;
use ipp_duplexd_fork::service;
use rust_i18n::t;
use std::io::{self, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

rust_i18n::i18n!("locales", fallback = "en");

static ASSUME_DEFAULTS: AtomicBool = AtomicBool::new(false);
static LANG: OnceLock<&'static str> = OnceLock::new();

/// The language everything in this run is printed in.
fn lang() -> &'static str {
    LANG.get().copied().unwrap_or(l10n::FALLBACK)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // The language has to be settled before anything else can be said about
    // the remaining arguments, so pick it out first.
    match arg_value(&argv, "--lang") {
        Some(tag) => match l10n::normalize(&tag) {
            Some(code) => {
                LANG.set(l10n::apply(code)).ok();
            }
            None => {
                eprintln!(
                    "{}",
                    l10n::tr(
                        l10n::FALLBACK,
                        "setup.err.bad_lang",
                        &[("lang", tag.clone()), ("list", l10n::describe_all())],
                    )
                );
                eprint!("{}", l10n::text(l10n::FALLBACK, "setup.usage"));
                std::process::exit(2);
            }
        },
        None => {
            let code = l10n::from_env().unwrap_or(l10n::FALLBACK);
            LANG.set(l10n::apply(code)).ok();
        }
    }

    let mut printer_uri: Option<String> = None;
    let mut queue_name: Option<String> = None;
    let mut listen = "127.0.0.1:6632".to_string();
    let mut do_service = true;
    let mut do_queue = true;

    let mut args = argv.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--printer" => printer_uri = args.next().cloned(),
            "--queue" => queue_name = args.next().cloned(),
            "--listen" => listen = args.next().cloned().unwrap_or(listen),
            "--lang" => {
                args.next(); // already handled above
            }
            "--no-service" => do_service = false,
            "--no-queue" => do_queue = false,
            "--yes" => ASSUME_DEFAULTS.store(true, Ordering::Relaxed),
            "-h" | "--help" => {
                print!("{}", t!("setup.usage"));
                return;
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

    println!("{}", t!("setup.title"));
    println!("=============================================");

    // 1. prerequisites -----------------------------------------------------
    if !have("qpdf") {
        println!();
        println!("{}", t!("setup.err.no_qpdf"));
        println!("{}", t!("setup.err.no_qpdf_hint"));
        if !confirm(&t!("setup.err.continue_anyway"), false) {
            std::process::exit(1);
        }
    }
    let daemon_bin = find_duplexd().unwrap_or_else(|| {
        println!("{}", t!("setup.err.no_daemon"));
        println!("{}", t!("setup.err.no_daemon_hint"));
        std::process::exit(1);
    });

    // 2. real printer ------------------------------------------------------
    println!();
    println!("{}", t!("setup.step.printer"));
    let printer_uri = printer_uri.unwrap_or_else(choose_printer);
    match ipp::parse_uri(&printer_uri) {
        Err(e) => {
            println!(
                "{}",
                t!("setup.printer.bad_uri", uri = printer_uri.as_str(), err = e)
            );
            std::process::exit(1);
        }
        Ok(uri) => match probe(&uri) {
            Some(mm) => println!("{}", t!("setup.printer.found", model = mm)),
            None => {
                println!(
                    "{}",
                    t!("setup.printer.no_answer", uri = printer_uri.as_str())
                );
                println!("{}", t!("setup.printer.no_answer_hint"));
                if !confirm(&t!("setup.printer.use_anyway"), true) {
                    std::process::exit(1);
                }
            }
        },
    }

    // 3. options + config file ---------------------------------------------
    println!();
    println!("{}", t!("setup.step.config"));
    let mut daemon_args = format!("--printer {printer_uri}");
    if listen != "127.0.0.1:6632" {
        daemon_args += &format!(" --listen {listen}");
    }
    // The service prints in the language chosen here unless the environment
    // says otherwise, so remember the choice in the config file.
    if lang() != l10n::FALLBACK {
        daemon_args += &format!(" --lang {}", lang());
    }
    let mut gui = true;
    if confirm(&t!("setup.ask.advanced"), false) {
        if !confirm(&t!("setup.ask.gui"), true) {
            daemon_args += " --no-gui";
            gui = false;
        }
        if !confirm(&t!("setup.ask.rotate"), false) {
            daemon_args += " --rotate-even 0";
        }
        let blank = ask(&t!("setup.ask.blank"), "none");
        if blank != "none" {
            daemon_args += &format!(" --blank {blank}");
        }
    }
    write_config(&daemon_args);

    // 4. service ------------------------------------------------------------
    println!();
    println!("{}", t!("setup.step.service"));
    let mut running = false;
    if do_service && have("systemctl") {
        running = setup_service(&daemon_bin, gui);
    } else if do_service {
        println!("{}", t!("setup.service.no_systemd"));
        println!("    {} {daemon_args}", daemon_bin.display());
    } else {
        println!("{}", t!("setup.service.skipped"));
    }

    // 5. verify the virtual printer ----------------------------------------
    let port = listen
        .rsplit(':')
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(6632);
    let self_uri = format!("ipp://127.0.0.1:{port}/ipp/print");
    if running {
        print!("{}", t!("setup.service.waiting", listen = listen.as_str()));
        io::stdout().flush().ok();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !running_at(port) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
            print!(".");
            io::stdout().flush().ok();
        }
        println!();
        match ipp::parse_uri(&self_uri).ok().and_then(|u| probe(&u)) {
            Some(mm) => println!("{}", t!("setup.service.up", model = mm)),
            None => println!("{}", t!("setup.service.down", listen = listen.as_str())),
        }
    }

    // 6. CUPS queue ----------------------------------------------------------
    println!();
    println!("{}", t!("setup.step.queue"));
    if do_queue && have("lpadmin") {
        let default = queue_default(&printer_uri);
        let queue = queue_name.unwrap_or_else(|| ask(&t!("setup.queue.name"), &default));
        add_queue(&queue, &self_uri);
    } else if do_queue {
        println!("{}", t!("setup.queue.no_lpadmin"));
        println!("    lpadmin -p manual-duplex -E -v {self_uri} -m everywhere");
    } else {
        println!("{}", t!("setup.queue.skipped"));
    }

    println!();
    println!("{}", t!("setup.done", port = port));
    println!("{}", t!("setup.done.status", port = port));
}

// ------------------------------------------------------------------- args

/// The value of `flag` in `argv`, wherever it appears.
fn arg_value(argv: &[String], flag: &str) -> Option<String> {
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().cloned();
        }
    }
    None
}

// ---------------------------------------------------------------- questions

fn ask(prompt: &str, default: &str) -> String {
    if ASSUME_DEFAULTS.load(Ordering::Relaxed) {
        return default.into();
    }
    print!("{prompt} [{default}]: ");
    io::stdout().flush().ok();
    let mut s = String::new();
    if io::stdin().read_line(&mut s).unwrap_or(0) == 0 {
        println!();
        return default.into(); // EOF
    }
    let t = s.trim();
    if t.is_empty() {
        default.into()
    } else {
        t.into()
    }
}

fn confirm(prompt: &str, default: bool) -> bool {
    let d = if default { "Y/n" } else { "y/N" };
    let a = ask(prompt, d);
    match a.to_ascii_lowercase().as_str() {
        "y" | "yes" | "是" => true,
        "n" | "no" | "否" => false,
        _ => default,
    }
}

// ------------------------------------------------------------------ helpers

fn have(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success() || !o.stderr.is_empty() || !o.stdout.is_empty())
        .unwrap_or(false)
}

fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let mut msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if msg.is_empty() {
            msg = format!("exit status {}", out.status);
        }
        Err(msg)
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

/// The daemon to install in the unit: the same lookup the window uses, so the
/// two front ends cannot disagree about which binary a machine runs.
fn find_duplexd() -> Option<PathBuf> {
    service::daemon_binary()
}

fn running_at(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// Get-Printer-Attributes; Some(make-and-model) if an IPP printer answers.
fn probe(uri: &ipp::PrinterUri) -> Option<String> {
    let mut m = Msg::new(ipp::OP_GET_PRINTER_ATTRS, 1);
    m.groups.push(Group {
        tag: ipp::G_OPERATION,
        attrs: vec![
            Attr::str(ipp::T_CHARSET, "attributes-charset", "utf-8"),
            Attr::str(ipp::T_LANGUAGE, "attributes-natural-language", "en"),
            Attr::str(ipp::T_URI, "printer-uri", &uri.uri),
        ],
    });
    let resp = ipp::request(uri, &m).ok()?;
    let unnamed = t!("setup.probe.unnamed").to_string();
    Some(
        resp.find("printer-make-and-model")
            .and_then(|a| a.first_str())
            .unwrap_or(unnamed.as_str())
            .to_string(),
    )
}

// ------------------------------------------------------------ printer choice

/// All CUPS queues as (name, device-uri), parsed from `lpstat -v`.
fn cups_queues() -> Vec<(String, String)> {
    let Ok(out) = run("lpstat", &["-v"]) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|l| {
            // "device for laser: ipp://host/..."
            let rest = l.strip_prefix("device for ")?;
            let (name, uri) = rest.split_once(':')?;
            Some((name.trim().to_string(), uri.trim().to_string()))
        })
        .collect()
}

fn choose_printer() -> String {
    // skip queues on loopback:6632 — that would be ipp-duplexd-fork forwarding to itself
    let queues: Vec<_> = cups_queues()
        .into_iter()
        .filter(|(_, uri)| !uri.contains("127.0.0.1:6632") && !uri.contains("localhost:6632"))
        .collect();
    if queues.is_empty() {
        println!("{}", t!("setup.choose.no_queues"));
        return ask(
            &t!("setup.choose.real_uri"),
            "ipp://192.168.1.100:631/ipp/print",
        );
    }
    println!("{}", t!("setup.choose.recommend"));
    for (i, (name, uri)) in queues.iter().enumerate() {
        println!("    {}) {name}  ({uri})", i + 1);
    }
    println!("{}", t!("setup.choose.or_uri"));
    loop {
        let a = ask(&t!("setup.choose.real_printer"), "1");
        if let Ok(n) = a.parse::<usize>() {
            if n >= 1 && n <= queues.len() {
                return format!("ipp://127.0.0.1:631/printers/{}", queues[n - 1].0);
            }
        }
        if a.starts_with("ipp://") || a.starts_with("http://") {
            return a;
        }
        println!("{}", t!("setup.choose.bad_input", n = queues.len()));
        if ASSUME_DEFAULTS.load(Ordering::Relaxed) {
            std::process::exit(1); // avoid looping forever with --yes
        }
    }
}

/// "laser-duplex" when forwarding to a CUPS queue, else "manual-duplex".
fn queue_default(printer_uri: &str) -> String {
    printer_uri
        .rsplit_once("/printers/")
        .map(|(_, q)| format!("{q}-duplex"))
        .unwrap_or_else(|| "manual-duplex".into())
}

// -------------------------------------------------------------- config file

fn write_config(daemon_args: &str) {
    let path = home().join(".config/ipp-duplexd-fork.conf");
    let line = format!("IPP_DUPLEXD_FORK_ARGS={daemon_args}\n");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if existing == line {
            println!("{}", t!("setup.config.up_to_date", path = path.display()));
            return;
        }
        println!("{}", t!("setup.config.exists", path = path.display()));
        for l in existing.lines().take(5) {
            println!("    | {l}");
        }
        if !confirm(&t!("setup.config.overwrite"), true) {
            println!("{}", t!("setup.config.keeping"));
            return;
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    match std::fs::write(&path, &line) {
        Ok(()) => println!(
            "{}",
            t!(
                "setup.config.wrote",
                path = path.display(),
                line = line.trim_end()
            )
        ),
        Err(e) => {
            println!(
                "{}",
                t!("setup.config.cannot_write", path = path.display(), err = e)
            );
            std::process::exit(1);
        }
    }
}

// ------------------------------------------------------------------ service

/// Enable + (re)start the user service; returns true if it should be running.
fn setup_service(daemon_bin: &Path, gui: bool) -> bool {
    // package-installed unit, or write one ourselves
    let packaged = Path::new("/usr/lib/systemd/user/ipp-duplexd-fork.service").is_file()
        || Path::new("/lib/systemd/user/ipp-duplexd-fork.service").is_file();
    let user_unit = home().join(".config/systemd/user/ipp-duplexd-fork.service");
    if !packaged && !user_unit.is_file() {
        // With the dialog, bind to the graphical session: it starts once the
        // display variables exist and stops when they go away. Without it,
        // there is nothing session-bound about the daemon, so run it for the
        // whole login (it still serves GET /flip over ssh, with no desktop up).
        let unit = service::unit_text(daemon_bin, gui);
        std::fs::create_dir_all(user_unit.parent().unwrap()).ok();
        match std::fs::write(&user_unit, unit) {
            Ok(()) => println!(
                "{}",
                t!("setup.service.installed", path = user_unit.display())
            ),
            Err(e) => {
                println!(
                    "{}",
                    t!(
                        "setup.service.cannot_write",
                        path = user_unit.display(),
                        err = e
                    )
                );
                return false;
            }
        }
        let _ = run("systemctl", &["--user", "daemon-reload"]);
    }
    // The dialog needs the session's display variables in the service
    // environment. A desktop session normally imports them into the user
    // manager itself at login; warn if this one did not, since the dialog will
    // not open until they are there.
    if gui {
        let manager_env = run("systemctl", &["--user", "show-environment"]).unwrap_or_default();
        let missing: Vec<String> = ["DISPLAY", "WAYLAND_DISPLAY"]
            .iter()
            .filter(|v| {
                !manager_env
                    .lines()
                    .any(|l| l.split('=').next() == Some(**v))
            })
            .filter_map(|v| {
                let val = std::env::var_os(v)?;
                Some(format!("{v}={}", val.to_string_lossy()))
            })
            .collect();
        if !missing.is_empty() {
            println!("{}", t!("setup.service.no_display"));
            println!("{}", t!("setup.service.no_display_hint"));
            println!("{}", t!("setup.service.no_display_conf"));
            for kv in &missing {
                println!("      {kv}");
            }
        }
    }
    match run(
        "systemctl",
        &["--user", "enable", "--now", "ipp-duplexd-fork"],
    ) {
        Ok(_) => {}
        Err(e) => {
            println!("{}", t!("setup.service.enable_failed", err = e));
            println!(
                "{}",
                t!("setup.service.start_manually", bin = daemon_bin.display())
            );
            return false;
        }
    }
    // restart in case it was already running with the old config
    let _ = run("systemctl", &["--user", "restart", "ipp-duplexd-fork"]);
    println!("{}", t!("setup.service.enabled"));
    true
}

// --------------------------------------------------------------- CUPS queue

fn add_queue(queue: &str, self_uri: &str) {
    // already pointing at us?
    if let Some((_, uri)) = cups_queues().into_iter().find(|(n, _)| n == queue) {
        if uri == self_uri {
            println!("{}", t!("setup.queue.already", queue = queue));
            return;
        }
        println!(
            "{}",
            t!("setup.queue.exists", queue = queue, uri = uri.as_str())
        );
        if !confirm(&t!("setup.queue.repoint"), false) {
            return;
        }
    }
    match run(
        "lpadmin",
        &["-p", queue, "-E", "-v", self_uri, "-m", "everywhere"],
    ) {
        Ok(_) => {
            println!("{}", t!("setup.queue.added", queue = queue, uri = self_uri));
            println!("{}", t!("setup.queue.test", queue = queue));
        }
        Err(e) => {
            println!("{}", t!("setup.queue.lpadmin_failed", err = e));
            println!("{}", t!("setup.queue.need_admin"));
            println!("    sudo lpadmin -p {queue} -E -v {self_uri} -m everywhere");
        }
    }
}
