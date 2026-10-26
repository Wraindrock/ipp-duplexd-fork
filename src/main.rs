//! ipp-duplexd-fork - a virtual IPP printer (loopback only) that performs manual
//! duplex printing on a real IPP printer.
//!
//! Jobs printed with sides=two-sided-* (the advertised default) are split
//! with qpdf: odd pages are printed first; when they are done the job stops
//! with reason media-needed until you flip the stack and hit GET /flip (or
//! --auto-continue elapses); then the even pages are printed in reverse
//! order. Jobs with sides=one-sided pass through untouched.
//!
//! Point it at any ipp:// endpoint; a local CUPS queue
//! (ipp://127.0.0.1:631/printers/<name>) works well because CUPS then does
//! any driver work. Printer attributes are proxied from the real printer,
//! so the virtual printer advertises the real hardware's capabilities.

use ipp_duplexd_fork::config as cfg_file;
use ipp_duplexd_fork::ipp::{self, Attr, Group, Msg, PrinterUri};
use ipp_duplexd_fork::{gui, l10n};
use rust_i18n::t;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

// The catalogs live in locales/ and are compiled in from there.
rust_i18n::i18n!("locales", fallback = "en");

// IPP job states
const J_PENDING: i32 = 3;
const J_PROCESSING: i32 = 5;
const J_STOPPED: i32 = 6;
const J_CANCELED: i32 = 7;
const J_ABORTED: i32 = 8;
const J_COMPLETED: i32 = 9;

#[derive(Clone)]
struct Settings {
    listen: String,
    printer: PrinterUri,
    name: String,
    blank: String,      // trailing | leading | none
    auto_continue: u64, // seconds; 0 = wait for /flip or the dialog
    rotate_even: u32,   // 0 | 180
    poll: u64,          // seconds between remote job-state polls
    gui: bool,          // open a desktop dialog to confirm the flip
    settle: u64,        // seconds to give the real printer before the backs
}

/// A job's state message.
///
/// IPP clients are told the message in the daemon's language; the status page
/// is rendered per request, so the key is kept until a page asks for it.
enum JobMessage {
    Text(String),
    Key(&'static str, Vec<(&'static str, String)>),
}

impl JobMessage {
    /// A catalog key without placeholders.
    fn key(key: &'static str) -> Self {
        JobMessage::Key(key, Vec::new())
    }

    /// A catalog key with `%{...}` placeholders.
    fn with(key: &'static str, vars: Vec<(&'static str, String)>) -> Self {
        JobMessage::Key(key, vars)
    }

    fn render(&self, locale: &str) -> String {
        match self {
            JobMessage::Text(text) => text.clone(),
            JobMessage::Key(key, vars) => l10n::tr(locale, key, vars),
        }
    }
}

struct Job {
    id: i32,
    name: String,
    user: String,
    state: i32,
    message: JobMessage,
    // set for Create-Job jobs awaiting their Send-Document
    pending_attrs: Option<Vec<Attr>>,
}

struct FlipGate {
    requested: Mutex<bool>,
    cv: Condvar,
}

struct Ctx {
    cfg: Settings,
    port: u16,
    reload: mpsc::Sender<()>,
    jobs: Mutex<HashMap<i32, Job>>,
    next_id: AtomicI32,
    flip: FlipGate,
    process_lock: Mutex<()>,
    // (fetched-at, Some(attrs) on success / None when unreachable)
    attr_cache: Mutex<Option<(Instant, Option<Vec<Attr>>)>>,
}

/// The daemon's own language, settled by `parse_args` before anything is
/// printed. `t!` reads the same value from rust-i18n's global locale; both are
/// set from the one place, `parse_args`.
static LANG: OnceLock<&'static str> = OnceLock::new();

fn lang() -> &'static str {
    LANG.get().copied().unwrap_or(l10n::FALLBACK)
}

fn log(msg: &str) {
    eprintln!("ipp-duplexd-fork: {msg}");
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&argv) {
        if !e.is_empty() {
            eprintln!("ipp-duplexd-fork: {e}");
        }
        eprintln!("{}", l10n::text(lang(), "daemon.usage"));
        std::process::exit(2);
    }
}

/// Read the command line's `--lang` first, so even the errors below are in the
/// language the caller asked for.
fn settle_language(argv: &[String]) -> Result<(), String> {
    let code = match arg_value(argv, "--lang") {
        Some(tag) => match l10n::normalize(&tag) {
            Some(code) => code,
            None => {
                return Err(l10n::tr(
                    l10n::FALLBACK,
                    "daemon.err.bad_lang",
                    &[("lang", tag), ("list", language_list())],
                ))
            }
        },
        None => l10n::from_env().unwrap_or(l10n::FALLBACK),
    };
    let _ = LANG.set(l10n::apply(code));
    Ok(())
}

fn run(argv: &[String]) -> Result<(), String> {
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        return Err(String::new());
    }
    settle_language(argv)?;
    if Command::new("qpdf").arg("--version").output().is_err() {
        eprintln!("ipp-duplexd-fork: {}", t!("daemon.err.no_qpdf"));
        std::process::exit(1);
    }
    // A configuration file wins over the single-printer flags; the default
    // location is used when it exists, so a machine that was migrated keeps
    // starting every printer it was told about.
    let file = match arg_value(argv, "--config") {
        Some(path) => Some(PathBuf::from(path)),
        None => {
            let default = cfg_file::Config::path();
            if default.is_file() {
                Some(default)
            } else {
                None
            }
        }
    };
    let (reload, reload_rx) = mpsc::channel::<()>();
    let mut daemon = Daemon {
        file: file.clone(),
        reload,
        running: HashMap::new(),
    };
    let printers = match &file {
        Some(path) => {
            let config = cfg_file::Config::load(path, lang())?.ok_or_else(|| {
                t!(
                    "daemon.err.config_missing",
                    path = path.display().to_string()
                )
                .to_string()
            })?;
            config.validate(lang())?;
            log(&t!(
                "daemon.log.config_loaded",
                count = config.printers.len(),
                path = path.display().to_string()
            ));
            config.printers
        }
        None => vec![legacy_printer(argv)?],
    };
    daemon.apply(printers)?;
    // The listener threads do the serving; this thread owns the printer list
    // and applies `POST /reload` requests coming from any of them.
    for _ in reload_rx {
        daemon.reload();
    }
    Ok(())
}

/// One virtual printer that is currently serving.
struct Running {
    stop: Arc<AtomicBool>,
    fingerprint: String,
}

/// The printers the process serves, plus the file it re-reads on reload.
struct Daemon {
    file: Option<PathBuf>,
    reload: mpsc::Sender<()>,
    running: HashMap<String, Running>,
}

/// The per-printer settings the workers use, built from a file entry.
fn settings_of(printer: &cfg_file::Printer) -> Result<Settings, String> {
    let uri = ipp::parse_uri(printer.real.trim()).map_err(|e| {
        t!(
            "daemon.err.bad_printer_uri",
            uri = printer.real.as_str(),
            err = e
        )
        .to_string()
    })?;
    Ok(Settings {
        listen: printer.listen.trim().to_string(),
        printer: uri,
        name: printer.name.trim().to_string(),
        blank: printer.blank.clone(),
        auto_continue: printer.auto_continue,
        rotate_even: printer.rotate_even,
        poll: printer.poll,
        gui: printer.gui,
        settle: printer.settle,
    })
}

impl Settings {
    /// A cheap comparison key, so a reload only restarts what changed.
    fn fingerprint(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.listen,
            self.printer.uri,
            self.name,
            self.blank,
            self.auto_continue,
            self.rotate_even,
            self.poll,
            self.gui,
            self.settle
        )
    }
}

fn bind_with_retry(addr: &str) -> Result<TcpListener, String> {
    let mut last = String::new();
    for _ in 0..20 {
        match TcpListener::bind(addr) {
            Ok(listener) => {
                let _ = listener.set_nonblocking(true);
                return Ok(listener);
            }
            Err(e) => {
                last = e.to_string();
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(t!("daemon.err.bind", addr = addr, err = last).to_string())
}

impl Daemon {
    /// Start one virtual printer: bind, log, and spawn its accept loop.
    fn start(&mut self, printer: &cfg_file::Printer) -> Result<(), String> {
        let settings = settings_of(printer)?;
        let listener = bind_with_retry(&settings.listen)?;
        let (addr, port) = match listener.local_addr() {
            Ok(a) => (a.to_string(), a.port()),
            Err(_) => (settings.listen.clone(), 631),
        };
        log(&t!(
            "daemon.log.listening",
            addr = addr.as_str(),
            name = settings.name.as_str(),
            uri = settings.printer.uri.as_str()
        ));
        log(&t!(
            "daemon.log.lpadmin",
            name = settings.name.as_str(),
            port = port
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let ctx = Arc::new(Ctx {
            cfg: settings.clone(),
            port,
            reload: self.reload.clone(),
            jobs: Mutex::new(HashMap::new()),
            next_id: AtomicI32::new(1),
            flip: FlipGate {
                requested: Mutex::new(false),
                cv: Condvar::new(),
            },
            process_lock: Mutex::new(()),
            attr_cache: Mutex::new(None),
        });
        serve(ctx, listener, stop.clone());
        self.running.insert(
            settings.name.clone(),
            Running {
                stop,
                fingerprint: settings.fingerprint(),
            },
        );
        Ok(())
    }

    /// Make the running printers match the wanted list.
    fn apply(&mut self, printers: Vec<cfg_file::Printer>) -> Result<(), String> {
        let mut wanted: HashMap<String, cfg_file::Printer> = HashMap::new();
        for printer in printers {
            wanted.insert(printer.name.trim().to_string(), printer);
        }
        let names: Vec<String> = self.running.keys().cloned().collect();
        for name in names {
            let keep = match wanted.get(&name) {
                Some(printer) => settings_of(printer)
                    .map(|settings| settings.fingerprint() == self.running[&name].fingerprint)
                    .unwrap_or(false),
                None => false,
            };
            if !keep {
                if let Some(running) = self.running.remove(&name) {
                    running.stop.store(true, Ordering::Relaxed);
                    log(&t!("daemon.log.printer_stopped", name = name.as_str()));
                }
            }
        }
        let mut wanted: Vec<cfg_file::Printer> = wanted.into_values().collect();
        wanted.sort_by(|a, b| a.name.cmp(&b.name));
        for printer in wanted {
            if self.running.contains_key(printer.name.trim()) {
                continue;
            }
            self.start(&printer)?;
        }
        Ok(())
    }

    /// Re-read the configuration file and apply it.
    fn reload(&mut self) {
        // Started from flags with no file: the window may have written one
        // since, and reload is exactly the moment to adopt it.
        if self.file.is_none() {
            let default = cfg_file::Config::path();
            if default.is_file() {
                self.file = Some(default);
            }
        }
        let path = match self.file.clone() {
            Some(path) => path,
            None => {
                log(&t!("daemon.log.reload_no_file"));
                return;
            }
        };
        let shown = path.display().to_string();
        let config = match cfg_file::Config::load(&path, lang()) {
            Ok(Some(config)) => config,
            Ok(None) => {
                log(&t!(
                    "daemon.log.reload_failed",
                    path = shown.as_str(),
                    err = t!("daemon.err.config_missing", path = shown.as_str()).to_string()
                ));
                return;
            }
            Err(e) => {
                log(&t!(
                    "daemon.log.reload_failed",
                    path = shown.as_str(),
                    err = e.as_str()
                ));
                return;
            }
        };
        if let Err(e) = config.validate(lang()) {
            log(&t!(
                "daemon.log.reload_failed",
                path = shown.as_str(),
                err = e.as_str()
            ));
            return;
        }
        let count = config.printers.len();
        match self.apply(config.printers) {
            Ok(()) => log(&t!(
                "daemon.log.reload_ok",
                path = shown.as_str(),
                count = count
            )),
            Err(e) => log(&t!(
                "daemon.log.reload_failed",
                path = shown.as_str(),
                err = e.as_str()
            )),
        }
    }
}

/// Accept connections until told to stop, one thread per connection.
fn serve(ctx: Arc<Ctx>, listener: TcpListener, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let ctx = ctx.clone();
                std::thread::spawn(move || handle_client(ctx, stream));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                log(&t!("daemon.log.accept_error", err = e.to_string()));
                return;
            }
        }
    });
}

/// The single printer described by the command line, for machines that have no
/// configuration file yet.
fn legacy_printer(argv: &[String]) -> Result<cfg_file::Printer, String> {
    let settings = config_from(argv)?;
    Ok(cfg_file::Printer {
        name: settings.name,
        listen: settings.listen,
        real: settings.printer.uri,
        blank: settings.blank,
        settle: settings.settle,
        poll: settings.poll,
        rotate_even: settings.rotate_even,
        auto_continue: settings.auto_continue,
        gui: settings.gui,
    })
}

/// `--flag value` lookup, for the one option that must be understood before
/// the rest of the command line is parsed.
fn arg_value(argv: &[String], flag: &str) -> Option<String> {
    let mut args = argv.iter();
    while let Some(a) = args.next() {
        if a == flag {
            return args.next().cloned();
        }
    }
    None
}

/// "en (English), zh-CN (简体中文)", for the unsupported-language error.
fn language_list() -> String {
    l10n::describe_all()
}

fn config_from(argv: &[String]) -> Result<Settings, String> {
    let mut args = argv.iter();
    let mut listen = "127.0.0.1:6632".to_string();
    let mut printer = None;
    let mut name = "manual-duplex".to_string();
    let mut blank = "leading".to_string();
    let mut auto_continue = 0u64;
    let mut rotate_even = 180u32;
    let mut poll = 2u64;
    let mut gui = true;
    let mut settle = 10u64;
    while let Some(a) = args.next() {
        let mut val = |what: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| t!("daemon.err.needs_value", what = what).to_string())
        };
        match a.as_str() {
            "--listen" => listen = val("--listen")?,
            "--printer" => {
                let uri = val("--printer")?;
                printer = Some(ipp::parse_uri(&uri).map_err(|e| {
                    t!("daemon.err.bad_printer_uri", uri = uri.as_str(), err = e).to_string()
                })?);
            }
            "--name" => name = val("--name")?,
            "--blank" => blank = val("--blank")?,
            "--auto-continue" => {
                auto_continue = val("--auto-continue")?
                    .parse()
                    .map_err(|_| t!("daemon.err.bad_auto_continue").to_string())?
            }
            "--rotate-even" => {
                rotate_even = val("--rotate-even")?
                    .parse()
                    .map_err(|_| t!("daemon.err.bad_rotate_even").to_string())?
            }
            "--settle" => {
                settle = val("--settle")?
                    .parse::<u64>()
                    .map_err(|_| t!("daemon.err.bad_settle").to_string())?
                    .min(600)
            }
            "--poll" => {
                poll = val("--poll")?
                    .parse::<u64>()
                    .map_err(|_| t!("daemon.err.bad_poll").to_string())?
                    .max(1)
            }
            // already applied, by parse_args
            "--lang" => {
                val("--lang")?;
            }
            "--gui" => gui = true,
            "--no-gui" => gui = false,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(t!("daemon.err.unknown_arg", arg = other).to_string()),
        }
    }
    if !matches!(blank.as_str(), "trailing" | "leading" | "none") {
        return Err(t!("daemon.err.bad_blank").to_string());
    }
    if !matches!(rotate_even, 0 | 180) {
        return Err(t!("daemon.err.bad_rotate_value").to_string());
    }
    if !listen.starts_with("127.")
        && !listen.starts_with("localhost:")
        && !listen.starts_with("[::1]")
    {
        log(&t!("daemon.log.nonloopback", addr = listen.as_str()));
    }
    Ok(Settings {
        listen,
        printer: printer.ok_or_else(|| t!("daemon.err.printer_required").to_string())?,
        name,
        blank,
        auto_continue,
        rotate_even,
        poll,
        gui,
        settle,
    })
}

// ---------------------------------------------------------------- HTTP layer

fn handle_client(ctx: Arc<Ctx>, stream: TcpStream) {
    stream.set_read_timeout(Some(Duration::from_secs(300))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(60))).ok();
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let mut reader = BufReader::new(stream);
    // Serve sequential requests on one connection (CUPS reuses it for
    // Create-Job + Send-Document).
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let mut parts = line.split_whitespace();
        let (method, path) = match (parts.next(), parts.next()) {
            (Some(m), Some(p)) => (m.to_string(), p.to_string()),
            _ => return,
        };
        let mut content_length: Option<usize> = None;
        let mut chunked = false;
        let mut expect_continue = false;
        let mut keep_alive = true;
        let mut accept_language = String::new();
        loop {
            let mut h = String::new();
            match reader.read_line(&mut h) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if h == "\r\n" || h == "\n" {
                break;
            }
            let lower = h.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:") {
                content_length = v.trim().parse().ok();
            } else if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
                chunked = true;
            } else if lower.starts_with("expect:") && lower.contains("100-continue") {
                expect_continue = true;
            } else if lower.starts_with("connection:") && lower.contains("close") {
                keep_alive = false;
            } else if let Some(v) = lower.strip_prefix("accept-language:") {
                accept_language = v.trim().to_string();
            }
        }
        if expect_continue {
            let _ = writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
            let _ = writer.flush();
        }
        let body = if method == "POST" {
            match ipp::read_body(&mut reader, content_length, chunked) {
                Ok(b) => b,
                Err(e) => {
                    log(&t!("daemon.log.bad_body", err = e.to_string()));
                    return;
                }
            }
        } else {
            Vec::new()
        };

        // The status page and the flip replies follow the browser's language;
        // IPP responses do not (see README, "What stays English").
        let lang = l10n::negotiate(&accept_language);
        let (status, ctype, resp): (&str, &str, Vec<u8>) = match (method.as_str(), path.as_str()) {
            ("POST", "/reload") => {
                let _ = ctx.reload.send(());
                (
                    "200 OK",
                    "text/plain",
                    format!("{}\n", t!("daemon.http.reloaded", locale = lang)).into_bytes(),
                )
            }
            ("POST", _) => match ipp::parse(&body) {
                Ok(req) => {
                    let resp = handle_ipp(&ctx, req);
                    ("200 OK", "application/ipp", resp.encode())
                }
                Err(e) => {
                    log(&t!("daemon.log.bad_ipp", err = e.as_str()));
                    ("400 Bad Request", "text/plain", e.into_bytes())
                }
            },
            ("GET", "/flip") => {
                let n = trigger_flip(&ctx);
                let msg = if n {
                    t!("daemon.http.flip_ok", locale = lang)
                } else {
                    t!("daemon.http.flip_none", locale = lang)
                };
                ("200 OK", "text/plain", format!("{msg}\n").into_bytes())
            }
            ("GET", _) => ("200 OK", "text/plain", status_page(&ctx, lang).into_bytes()),
            _ => (
                "405 Method Not Allowed",
                "text/plain",
                format!("{}\n", t!("daemon.http.method_not_allowed", locale = lang)).into_bytes(),
            ),
        };
        let conn = if keep_alive { "keep-alive" } else { "close" };
        // Say which language a text response came out in, for caches/proxies.
        let language = if ctype == "text/plain" {
            format!("Content-Language: {lang}\r\n")
        } else {
            String::new()
        };
        let _ = write!(
            writer,
            "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: {conn}\r\n{language}\r\n",
            resp.len()
        );
        let _ = writer.write_all(&resp);
        let _ = writer.flush();
        if !keep_alive {
            return;
        }
    }
}

fn status_page(ctx: &Ctx, lang: &str) -> String {
    let jobs = ctx.jobs.lock().unwrap();
    let mut ids: Vec<_> = jobs.keys().copied().collect();
    ids.sort();
    let mut out = l10n::tr(
        lang,
        "daemon.page.header",
        &[
            ("name", ctx.cfg.name.clone()),
            ("uri", ctx.cfg.printer.uri.clone()),
        ],
    );
    out.push('\n');
    out.push_str(&l10n::text(lang, "daemon.page.hint"));
    out.push_str("\n\n");
    for id in ids {
        let j = &jobs[&id];
        out.push_str(&l10n::tr(
            lang,
            "daemon.page.job",
            &[
                ("id", j.id.to_string()),
                ("name", j.name.clone()),
                ("user", j.user.clone()),
                ("state", l10n::text(lang, state_key(j.state))),
                ("message", j.message.render(lang)),
            ],
        ));
        out.push('\n');
    }
    out
}

/// The catalog key naming an IPP job state.
fn state_key(s: i32) -> &'static str {
    match s {
        J_PENDING => "daemon.state.pending",
        4 => "daemon.state.pending_held",
        J_PROCESSING => "daemon.state.processing",
        J_STOPPED => "daemon.state.stopped",
        J_CANCELED => "daemon.state.canceled",
        J_ABORTED => "daemon.state.aborted",
        J_COMPLETED => "daemon.state.completed",
        _ => "daemon.state.unknown",
    }
}

fn trigger_flip(ctx: &Ctx) -> bool {
    let waiting = ctx
        .jobs
        .lock()
        .unwrap()
        .values()
        .any(|j| j.state == J_STOPPED);
    let mut req = ctx.flip.requested.lock().unwrap();
    *req = true;
    ctx.flip.cv.notify_all();
    waiting
}

// ----------------------------------------------------------------- IPP layer

fn handle_ipp(ctx: &Arc<Ctx>, req: Msg) -> Msg {
    let mut resp = match req.op_status {
        ipp::OP_GET_PRINTER_ATTRS => op_get_printer_attrs(ctx, &req),
        ipp::OP_VALIDATE_JOB => Msg::response(ipp::OK, req.request_id),
        ipp::OP_PRINT_JOB => op_print_job(ctx, req),
        ipp::OP_CREATE_JOB => op_create_job(ctx, &req),
        ipp::OP_SEND_DOCUMENT => op_send_document(ctx, req),
        ipp::OP_GET_JOBS => op_get_jobs(ctx, &req),
        ipp::OP_GET_JOB_ATTRS => op_get_job_attrs(ctx, &req),
        ipp::OP_CANCEL_JOB => op_cancel_job(ctx, &req),
        other => {
            log(&t!(
                "daemon.log.unsupported_op",
                op = format!("{other:04x}")
            ));
            Msg::response(ipp::ERR_OP_NOT_SUPPORTED, req.request_id)
        }
    };
    // The text a response carries (job-state-message, printer-state-message,
    // printer-info, ...) is generated in the daemon's language, so declare that
    // language instead of claiming English or echoing the real printer's claim.
    // RFC 8011 Section 5.1.9 wants naturalLanguage values all lowercase, and
    // CUPS refuses a printer whose values are not, so `--lang zh-CN` becomes
    // `zh-cn` here.
    let language = l10n::ipp_tag(lang());
    for group in resp.groups.iter_mut() {
        for attr in group.attrs.iter_mut() {
            if matches!(
                attr.name.as_str(),
                "attributes-natural-language"
                    | "natural-language-configured"
                    | "generated-natural-language-supported"
            ) {
                let name = attr.name.clone();
                *attr = Attr::str(ipp::T_LANGUAGE, &name, &language);
            }
        }
    }
    resp
}

fn req_user(req: &Msg) -> String {
    req.find_in(ipp::G_OPERATION, "requesting-user-name")
        .and_then(|a| a.first_str())
        .unwrap_or("anonymous")
        .to_string()
}

fn req_job_name(req: &Msg) -> String {
    req.find_in(ipp::G_OPERATION, "job-name")
        .or_else(|| req.find_in(ipp::G_JOB, "job-name"))
        .and_then(|a| a.first_str())
        .unwrap_or("untitled")
        .to_string()
}

fn job_attrs_of(req: &Msg) -> Vec<Attr> {
    req.groups
        .iter()
        .filter(|g| g.tag == ipp::G_JOB)
        .flat_map(|g| g.attrs.iter().cloned())
        .collect()
}

fn job_group(ctx: &Ctx, id: i32, state: i32, message: &str) -> Group {
    Group {
        tag: ipp::G_JOB,
        attrs: vec![
            Attr::int(ipp::T_INTEGER, "job-id", id),
            Attr::str(
                ipp::T_URI,
                "job-uri",
                &format!("ipp://127.0.0.1:{}/jobs/{id}", ctx.port),
            ),
            Attr::int(ipp::T_ENUM, "job-state", state),
            Attr::str(ipp::T_KEYWORD, "job-state-reasons", job_reason(state)),
            Attr::str(ipp::T_TEXT, "job-state-message", message),
        ],
    }
}

fn job_reason(state: i32) -> &'static str {
    match state {
        J_STOPPED => "media-needed",
        J_COMPLETED => "job-completed-successfully",
        J_CANCELED => "job-canceled-by-user",
        J_ABORTED => "aborted-by-system",
        _ => "none",
    }
}

fn op_print_job(ctx: &Arc<Ctx>, req: Msg) -> Msg {
    if req.data.is_empty() {
        return Msg::response(ipp::ERR_BAD_REQUEST, req.request_id);
    }
    let id = new_job(ctx, req_job_name(&req), req_user(&req), None);
    let attrs = job_attrs_of(&req);
    spawn_worker(ctx.clone(), id, req.data.clone(), attrs);
    let mut resp = Msg::response(ipp::OK, req.request_id);
    resp.groups.push(job_group(ctx, id, J_PENDING, ""));
    resp
}

fn op_create_job(ctx: &Arc<Ctx>, req: &Msg) -> Msg {
    let id = new_job(
        ctx,
        req_job_name(req),
        req_user(req),
        Some(job_attrs_of(req)),
    );
    let mut resp = Msg::response(ipp::OK, req.request_id);
    resp.groups.push(job_group(ctx, id, J_PENDING, ""));
    resp
}

fn op_send_document(ctx: &Arc<Ctx>, req: Msg) -> Msg {
    let id = match req
        .find_in(ipp::G_OPERATION, "job-id")
        .and_then(|a| a.first_int())
    {
        Some(i) => i,
        None => return Msg::response(ipp::ERR_BAD_REQUEST, req.request_id),
    };
    let last = req
        .find_in(ipp::G_OPERATION, "last-document")
        .and_then(|a| a.values.first().map(|(_, v)| v == &[1u8]))
        .unwrap_or(true);
    let attrs = {
        let mut jobs = ctx.jobs.lock().unwrap();
        let job = match jobs.get_mut(&id) {
            Some(j) => j,
            None => return Msg::response(ipp::ERR_NOT_FOUND, req.request_id),
        };
        match job.pending_attrs.take() {
            Some(a) => a,
            None => return Msg::response(ipp::ERR_NOT_POSSIBLE, req.request_id),
        }
    };
    if !last {
        // multi-document jobs are not supported
        set_job_state(
            ctx,
            id,
            J_ABORTED,
            JobMessage::key("daemon.job.multi_document"),
        );
        return Msg::response(ipp::ERR_OP_NOT_SUPPORTED, req.request_id);
    }
    if req.data.is_empty() {
        set_job_state(
            ctx,
            id,
            J_ABORTED,
            JobMessage::key("daemon.job.empty_document"),
        );
        return Msg::response(ipp::ERR_BAD_REQUEST, req.request_id);
    }
    spawn_worker(ctx.clone(), id, req.data.clone(), attrs);
    let mut resp = Msg::response(ipp::OK, req.request_id);
    resp.groups.push(job_group(ctx, id, J_PENDING, ""));
    resp
}

fn op_get_jobs(ctx: &Ctx, req: &Msg) -> Msg {
    let which = req
        .find_in(ipp::G_OPERATION, "which-jobs")
        .and_then(|a| a.first_str())
        .unwrap_or("not-completed")
        .to_string();
    let mut resp = Msg::response(ipp::OK, req.request_id);
    let jobs = ctx.jobs.lock().unwrap();
    let mut ids: Vec<_> = jobs.keys().copied().collect();
    ids.sort();
    for id in ids {
        let j = &jobs[&id];
        let done = j.state >= J_CANCELED;
        if (which == "completed") != done {
            continue;
        }
        let mut g = job_group_from(ctx, j);
        g.attrs.push(Attr::str(ipp::T_NAME, "job-name", &j.name));
        g.attrs
            .push(Attr::str(ipp::T_NAME, "job-originating-user-name", &j.user));
        resp.groups.push(g);
    }
    resp
}

fn job_group_from(ctx: &Ctx, j: &Job) -> Group {
    // IPP clients get the daemon's language whatever they asked for: one
    // job-state-message is shared by every client of this job.
    job_group(ctx, j.id, j.state, &j.message.render(lang()))
}

fn op_get_job_attrs(ctx: &Ctx, req: &Msg) -> Msg {
    let id = req
        .find_in(ipp::G_OPERATION, "job-id")
        .and_then(|a| a.first_int());
    let jobs = ctx.jobs.lock().unwrap();
    match id.and_then(|i| jobs.get(&i)) {
        Some(j) => {
            let mut resp = Msg::response(ipp::OK, req.request_id);
            let mut g = job_group_from(ctx, j);
            g.attrs.push(Attr::str(ipp::T_NAME, "job-name", &j.name));
            g.attrs
                .push(Attr::str(ipp::T_NAME, "job-originating-user-name", &j.user));
            resp.groups.push(g);
            resp
        }
        None => Msg::response(ipp::ERR_NOT_FOUND, req.request_id),
    }
}

fn op_cancel_job(ctx: &Ctx, req: &Msg) -> Msg {
    let id = match req
        .find_in(ipp::G_OPERATION, "job-id")
        .and_then(|a| a.first_int())
    {
        Some(i) => i,
        None => return Msg::response(ipp::ERR_BAD_REQUEST, req.request_id),
    };
    let mut jobs = ctx.jobs.lock().unwrap();
    match jobs.get_mut(&id) {
        Some(j) if j.state < J_CANCELED => {
            j.state = J_CANCELED;
            j.message = JobMessage::key("daemon.job.canceled");
            drop(jobs);
            // wake a worker that may be waiting for the flip
            ctx.flip.cv.notify_all();
            Msg::response(ipp::OK, req.request_id)
        }
        Some(_) => Msg::response(ipp::ERR_NOT_POSSIBLE, req.request_id),
        None => Msg::response(ipp::ERR_NOT_FOUND, req.request_id),
    }
}

// ------------------------------------------------- printer attributes proxy

fn op_get_printer_attrs(ctx: &Ctx, req: &Msg) -> Msg {
    let proxied = proxied_printer_attrs(ctx);
    let online = proxied.is_some();
    let mut attrs = proxied.unwrap_or_else(fallback_printer_attrs);
    apply_overrides(ctx, &mut attrs, online);
    let mut resp = Msg::response(ipp::OK, req.request_id);
    resp.groups.push(Group {
        tag: ipp::G_PRINTER,
        attrs,
    });
    resp
}

fn proxied_printer_attrs(ctx: &Ctx) -> Option<Vec<Attr>> {
    {
        // successes are cached for 30 s, failures for 5 s (so an
        // unreachable printer does not stall every attribute query)
        let cache = ctx.attr_cache.lock().unwrap();
        if let Some((t, result)) = cache.as_ref() {
            let ttl = if result.is_some() { 30 } else { 5 };
            if t.elapsed() < Duration::from_secs(ttl) {
                return result.clone();
            }
        }
    }
    let mut req = Msg::new(ipp::OP_GET_PRINTER_ATTRS, 1);
    req.groups.push(Group {
        tag: ipp::G_OPERATION,
        attrs: vec![
            Attr::str(ipp::T_CHARSET, "attributes-charset", "utf-8"),
            Attr::str(ipp::T_LANGUAGE, "attributes-natural-language", "en"),
            Attr::str(ipp::T_URI, "printer-uri", &ctx.cfg.printer.uri),
            Attr::str(ipp::T_KEYWORD, "requested-attributes", "all"),
        ],
    });
    match ipp::request(&ctx.cfg.printer, &req) {
        Ok(resp) => {
            let attrs: Vec<Attr> = resp
                .groups
                .iter()
                .filter(|g| g.tag == ipp::G_PRINTER)
                .flat_map(|g| g.attrs.iter().cloned())
                .collect();
            let result = if attrs.is_empty() { None } else { Some(attrs) };
            *ctx.attr_cache.lock().unwrap() = Some((Instant::now(), result.clone()));
            result
        }
        Err(e) => {
            log(&t!("daemon.log.attr_fetch_failed", err = e.as_str()));
            *ctx.attr_cache.lock().unwrap() = Some((Instant::now(), None));
            None
        }
    }
}

fn fallback_printer_attrs() -> Vec<Attr> {
    // The printer generates its text in exactly one language - the one `--lang`
    // or IPP_DUPLEXD_FORK_LANG picked - so that is the single value of both
    // language attributes; RFC 8011 Section 5.1.9 forbids the mixed-case
    // spelling our catalog codes use.
    let language = l10n::ipp_tag(lang());
    vec![
        Attr::str(ipp::T_CHARSET, "charset-configured", "utf-8"),
        Attr::str(ipp::T_CHARSET, "charset-supported", "utf-8"),
        Attr::str(ipp::T_LANGUAGE, "natural-language-configured", &language),
        Attr::str(
            ipp::T_LANGUAGE,
            "generated-natural-language-supported",
            &language,
        ),
        Attr::strs(ipp::T_KEYWORD, "ipp-versions-supported", &["1.1", "2.0"]),
        Attr::strs(ipp::T_KEYWORD, "pdl-override-supported", &["attempted"]),
        Attr::strs(
            ipp::T_KEYWORD,
            "media-supported",
            &["iso_a4_210x297mm", "na_letter_8.5x11in"],
        ),
        Attr::str(ipp::T_KEYWORD, "media-default", "iso_a4_210x297mm"),
        Attr::str(
            ipp::T_TEXT,
            "printer-make-and-model",
            "ipp-duplexd-fork virtual printer",
        ),
    ]
}

fn apply_overrides(ctx: &Ctx, attrs: &mut Vec<Attr>, online: bool) {
    // capture the real printer's status before stripping, so the virtual
    // printer mirrors it (online/idle/processing/stopped, supply alerts...)
    let real_state = attrs
        .iter()
        .find(|a| a.name == "printer-state")
        .and_then(|a| a.first_int());
    let real_reasons = attrs
        .iter()
        .find(|a| a.name == "printer-state-reasons")
        .cloned();
    let real_message = attrs
        .iter()
        .find(|a| a.name == "printer-state-message")
        .cloned();
    let real_accepting = attrs
        .iter()
        .find(|a| a.name == "printer-is-accepting-jobs")
        .and_then(|a| a.values.first().map(|(_, v)| v.as_slice() != [0u8]));
    let overridden = [
        "printer-uri-supported",
        "uri-security-supported",
        "uri-authentication-supported",
        "printer-name",
        "printer-uuid",
        "printer-state",
        "printer-state-reasons",
        "printer-state-message",
        "printer-is-accepting-jobs",
        "sides-supported",
        "sides-default",
        "operations-supported",
        "document-format-supported",
        "document-format-default",
        "printer-info",
        "printer-location",
        "printer-more-info",
        "queued-job-count",
        "printer-icons",
        "printer-device-id",
        "printer-supply-info-uri",
        "device-uri",
        "printer-alert",
        "printer-alert-description",
    ];
    attrs.retain(|a| !overridden.contains(&a.name.as_str()));

    let (stopped, processing, queued) = {
        let jobs = ctx.jobs.lock().unwrap();
        let active: Vec<_> = jobs.values().filter(|j| j.state < J_CANCELED).collect();
        (
            active.iter().any(|j| j.state == J_STOPPED),
            active.iter().any(|j| j.state == J_PROCESSING),
            active.len() as i32,
        )
    };
    // our own duplex flow wins; otherwise mirror the real printer's state
    let (state, reasons_attr, message_attr) = if stopped {
        (
            5,
            Attr::str(ipp::T_KEYWORD, "printer-state-reasons", "media-needed"),
            Attr::str(
                ipp::T_TEXT,
                "printer-state-message",
                &t!("daemon.attr.need_flip"),
            ),
        )
    } else if !online {
        (
            5,
            Attr::str(ipp::T_KEYWORD, "printer-state-reasons", "offline-report"),
            Attr::str(
                ipp::T_TEXT,
                "printer-state-message",
                &t!("daemon.attr.offline"),
            ),
        )
    } else {
        let mirrored = real_state.filter(|s| (3..=5).contains(s)).unwrap_or(3);
        let state = if processing { 4 } else { mirrored };
        (
            state,
            real_reasons
                .unwrap_or_else(|| Attr::str(ipp::T_KEYWORD, "printer-state-reasons", "none")),
            real_message.unwrap_or_else(|| Attr::str(ipp::T_TEXT, "printer-state-message", "")),
        )
    };
    let uri = format!("ipp://127.0.0.1:{}/ipp/print", ctx.port);
    let make = attrs
        .iter()
        .find(|a| a.name == "printer-make-and-model")
        .and_then(|a| a.first_str())
        .unwrap_or("printer")
        .to_string();
    attrs.retain(|a| a.name != "printer-make-and-model");
    attrs.extend([
        Attr::str(ipp::T_URI, "printer-uri-supported", &uri),
        Attr::str(ipp::T_KEYWORD, "uri-security-supported", "none"),
        Attr::str(ipp::T_KEYWORD, "uri-authentication-supported", "none"),
        Attr::str(ipp::T_NAME, "printer-name", &ctx.cfg.name),
        Attr::str(ipp::T_URI, "printer-uuid", &printer_uuid(&ctx.cfg.name)),
        Attr::int(ipp::T_ENUM, "printer-state", state),
        reasons_attr,
        message_attr,
        // keep accepting jobs even when the real printer is offline: we
        // are a spooler, and rejecting would fail the CUPS job outright
        Attr::boolean(
            "printer-is-accepting-jobs",
            real_accepting.unwrap_or(true) || !online,
        ),
        Attr::int(ipp::T_INTEGER, "queued-job-count", queued),
        Attr::strs(
            ipp::T_KEYWORD,
            "sides-supported",
            &["one-sided", "two-sided-long-edge", "two-sided-short-edge"],
        ),
        Attr::str(ipp::T_KEYWORD, "sides-default", "two-sided-long-edge"),
        Attr::ints(
            ipp::T_ENUM,
            "operations-supported",
            &[
                ipp::OP_PRINT_JOB as i32,
                ipp::OP_VALIDATE_JOB as i32,
                ipp::OP_CREATE_JOB as i32,
                ipp::OP_SEND_DOCUMENT as i32,
                ipp::OP_CANCEL_JOB as i32,
                ipp::OP_GET_JOB_ATTRS as i32,
                ipp::OP_GET_JOBS as i32,
                ipp::OP_GET_PRINTER_ATTRS as i32,
            ],
        ),
        Attr::strs(
            ipp::T_MIME,
            "document-format-supported",
            &["application/pdf", "application/octet-stream"],
        ),
        Attr::str(ipp::T_MIME, "document-format-default", "application/pdf"),
        Attr::str(
            ipp::T_TEXT,
            "printer-make-and-model",
            &t!("daemon.attr.make_and_model", make = make.as_str()),
        ),
        Attr::str(
            ipp::T_TEXT,
            "printer-info",
            &t!("daemon.attr.info", make = make.as_str()),
        ),
        Attr::str(ipp::T_TEXT, "printer-location", "localhost"),
    ]);
}

/// Deterministic urn:uuid derived from the printer name (FNV-1a based).
fn printer_uuid(name: &str) -> String {
    let mut h1: u64 = 0xcbf29ce484222325;
    let mut h2: u64 = 0x811c9dc5811c9dc5;
    for b in name.bytes() {
        h1 = (h1 ^ b as u64).wrapping_mul(0x100000001b3);
        h2 = (h2 ^ b as u64).wrapping_mul(0x1000193);
    }
    let b = [h1.to_be_bytes(), h2.to_be_bytes()].concat();
    format!(
        "urn:uuid:{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:01x}{:02x}-8{:01x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6] & 0x0f, b[7], b[8] & 0x0f, b[9],
        b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

// ------------------------------------------------------------------- worker

fn new_job(ctx: &Ctx, name: String, user: String, pending_attrs: Option<Vec<Attr>>) -> i32 {
    let id = ctx.next_id.fetch_add(1, Ordering::SeqCst);
    ctx.jobs.lock().unwrap().insert(
        id,
        Job {
            id,
            name,
            user,
            state: J_PENDING,
            message: JobMessage::Text(String::new()),
            pending_attrs,
        },
    );
    id
}

fn set_job_state(ctx: &Ctx, id: i32, state: i32, message: JobMessage) {
    if let Some(j) = ctx.jobs.lock().unwrap().get_mut(&id) {
        // never resurrect a canceled job
        if j.state == J_CANCELED && state != J_CANCELED {
            return;
        }
        j.state = state;
        j.message = message;
    }
}

fn job_canceled(ctx: &Ctx, id: i32) -> bool {
    ctx.jobs
        .lock()
        .unwrap()
        .get(&id)
        .map(|j| j.state == J_CANCELED)
        .unwrap_or(true)
}

fn spawn_worker(ctx: Arc<Ctx>, id: i32, data: Vec<u8>, attrs: Vec<Attr>) {
    std::thread::spawn(move || {
        let _serial = ctx.process_lock.lock().unwrap();
        if job_canceled(&ctx, id) {
            return;
        }
        match process_job(&ctx, id, data, attrs) {
            Ok(()) => {}
            Err(e) => {
                log(&t!("daemon.log.job_failed", id = id, err = e.as_str()));
                set_job_state(&ctx, id, J_ABORTED, JobMessage::Text(e));
            }
        }
    });
}

fn qpdf(args: &[&str]) -> Result<(), String> {
    let out = Command::new("qpdf")
        .arg("--warning-exit-0")
        .args(args)
        .output()
        .map_err(|e| t!("daemon.err.qpdf_spawn", err = e.to_string()).to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(t!(
            "daemon.log.qpdf_output",
            args = format!("{args:?}"),
            output = String::from_utf8_lossy(&out.stderr).trim()
        )
        .to_string())
    }
}

fn qpdf_npages(file: &str) -> Result<u32, String> {
    let out = Command::new("qpdf")
        .args(["--warning-exit-0", "--show-npages", file])
        .output()
        .map_err(|e| t!("daemon.err.qpdf_spawn", err = e.to_string()).to_string())?;
    if !out.status.success() {
        return Err(t!("daemon.err.qpdf_read").to_string());
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|_| t!("daemon.err.bad_page_count").to_string())
}

const BLANK_PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>\nendobj\nxref\n0 4\n0000000000 65535 f \ntrailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n0\n%%EOF\n";

fn attr_first_str<'a>(attrs: &'a [Attr], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|a| a.name == name)
        .and_then(|a| a.first_str())
}

/// Kills a still-running flip dialog when dropped.
struct DialogGuard(Option<Arc<Mutex<Option<std::process::Child>>>>);
impl Drop for DialogGuard {
    fn drop(&mut self) {
        if let Some(h) = &self.0 {
            if let Some(mut child) = h.lock().unwrap().take() {
                let _ = child.kill();
            }
        }
    }
}

/// Open a desktop "flip the stack" dialog with the first available tool.
/// OK triggers the flip; any other outcome (Cancel, closed window, no
/// display) just logs, so a broken GUI can never cancel a print job —
/// GET /flip keeps working in parallel.
/// The helper programs take their words as separate arguments; the GTK window
/// gets the same strings through `--flip`/`--lang`.
fn argument_list(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

fn spawn_flip_dialog(ctx: &Arc<Ctx>, job: &str) -> Option<Arc<Mutex<Option<std::process::Child>>>> {
    let text = t!("daemon.dialog.text", job = job).to_string();
    let ok = t!("daemon.dialog.ok").to_string();
    let not_yet = t!("daemon.dialog.not_yet").to_string();
    let buttons = format!("{ok}:101,{not_yet}:102");
    let flip = vec![
        "--flip".to_string(),
        job.to_string(),
        "--lang".to_string(),
        lang().to_string(),
    ];
    // This fork's own GTK4 dialog comes first, then the desktop tools the
    // daemon grew up with. Every candidate answers the same way: exit code
    // zero means "the stack is flipped, print the backs" (xmessage's button
    // code is 101). A binary that cannot run at all (no display, no GTK
    // build) fails to spawn or exits non-zero, which is always "not yet".
    let mut candidates: Vec<(&str, String, Vec<String>)> = Vec::new();
    if let Some(helper) = std::env::current_exe()
        .ok()
        .and_then(|exe| gui::find_helper(&exe))
    {
        candidates.push((
            gui::BIN,
            helper.to_string_lossy().into_owned(),
            flip.clone(),
        ));
    }
    // Installed into PATH, but not next to the daemon (or run out of a build
    // tree): the bare name lets the shell find it.
    candidates.push((gui::BIN, gui::BIN.to_string(), flip));
    candidates.push((
        "zenity",
        "zenity".to_string(),
        argument_list(&[
            "--question",
            "--title",
            gui::NAME,
            "--text",
            &text,
            "--ok-label",
            &ok,
            "--cancel-label",
            &not_yet,
        ]),
    ));
    candidates.push((
        "kdialog",
        "kdialog".to_string(),
        argument_list(&["--title", gui::NAME, "--yesno", &text]),
    ));
    candidates.push((
        "xmessage",
        "xmessage".to_string(),
        argument_list(&["-center", "-buttons", &buttons, &text]),
    ));
    let handle: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
    let h = handle.clone();
    let ctx = ctx.clone();
    let candidates: Arc<Vec<(&'static str, String, Vec<String>)>> = Arc::new(candidates);
    // The dialog runs in its own thread so a dismissal cannot block the job;
    // the daemon only reacts to the exit code.
    std::thread::spawn(move || {
        let mut index = 0;
        loop {
            // the first candidate that actually starts gets to answer
            let mut started = None;
            while index < candidates.len() {
                let (display, program, args) = &candidates[index];
                match Command::new(program).args(args).spawn() {
                    Ok(child) => {
                        started = Some((index, *display, child));
                        break;
                    }
                    Err(_) => index += 1,
                }
            }
            let Some((at, display, child)) = started else {
                log(&t!("daemon.log.no_dialog"));
                *h.lock().unwrap() = None;
                return;
            };
            log(&t!("daemon.log.dialog_opened", bin = display));
            *h.lock().unwrap() = Some(child);
            loop {
                let status = {
                    let mut g = h.lock().unwrap();
                    let Some(c) = g.as_mut() else { return }; // killed by the guard
                    match c.try_wait() {
                        Ok(Some(status)) => {
                            *g = None;
                            Some(status)
                        }
                        Ok(None) => None,
                        Err(_) => {
                            *g = None;
                            return;
                        }
                    }
                };
                let Some(status) = status else {
                    std::thread::sleep(Duration::from_millis(300));
                    continue;
                };
                // A dialog that could not even run is not a "no": a GTK
                // window without a display, or a helper that dies on startup,
                // falls through to the next tool instead of leaving the user
                // with no window at all.
                let could_not_run = match status.code() {
                    Some(code) => display == gui::BIN && code == i32::from(gui::EXIT_NO_DISPLAY),
                    None => true,
                };
                if could_not_run {
                    log(&t!("daemon.log.dialog_failed", bin = display));
                    index = at + 1;
                    break;
                }
                let accepted = if display == "xmessage" {
                    status.code() == Some(101)
                } else {
                    status.success()
                };
                if accepted {
                    trigger_flip(&ctx);
                } else {
                    log(&t!("daemon.log.dialog_dismissed"));
                }
                return;
            }
        }
    });
    Some(handle)
}

fn process_job(ctx: &Arc<Ctx>, id: i32, data: Vec<u8>, attrs: Vec<Attr>) -> Result<(), String> {
    set_job_state(
        ctx,
        id,
        J_PROCESSING,
        JobMessage::key("daemon.job.preparing"),
    );
    // pid in the name: two ipp-duplexd-fork instances both have a job 1
    let dir =
        std::env::temp_dir().join(format!("ipp-duplexd-fork-{}-job-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _cleanup = Cleanup(dir.clone());
    let p = |n: &str| dir.join(n).to_string_lossy().into_owned();

    std::fs::write(p("in.pdf"), &data).map_err(|e| e.to_string())?;
    let mut doc = p("in.pdf");

    // page-ranges: apply ourselves (rangeOfInteger values, possibly several)
    if let Some(a) = attrs.iter().find(|a| a.name == "page-ranges") {
        let mut ranges = vec![];
        for (_, v) in &a.values {
            if v.len() == 8 {
                let lo = i32::from_be_bytes(v[0..4].try_into().unwrap());
                let hi = i32::from_be_bytes(v[4..8].try_into().unwrap());
                ranges.push(format!("{lo}-{hi}"));
            }
        }
        if !ranges.is_empty() {
            qpdf(&[
                &doc,
                "--pages",
                ".",
                &ranges.join(","),
                "--",
                &p("ranged.pdf"),
            ])?;
            doc = p("ranged.pdf");
        }
    }

    // copies: replicate so one flip covers all copies, collated
    let copies = attrs
        .iter()
        .find(|a| a.name == "copies")
        .and_then(|a| a.first_int())
        .unwrap_or(1)
        .clamp(1, 999);
    if copies > 1 {
        let mut args: Vec<String> = vec!["--empty".into(), "--pages".into()];
        for _ in 0..copies {
            args.push(doc.clone());
            args.push("1-z".into());
        }
        args.push("--".into());
        args.push(p("copies.pdf"));
        let argrefs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        qpdf(&argrefs)?;
        doc = p("copies.pdf");
    }

    let sides = attr_first_str(&attrs, "sides").unwrap_or("two-sided-long-edge");
    let job_name = ctx
        .jobs
        .lock()
        .unwrap()
        .get(&id)
        .map(|j| j.name.clone())
        .unwrap_or_default();
    let user = ctx
        .jobs
        .lock()
        .unwrap()
        .get(&id)
        .map(|j| j.user.clone())
        .unwrap_or_default();

    if sides == "one-sided" {
        set_job_state(
            ctx,
            id,
            J_PROCESSING,
            JobMessage::key("daemon.job.pass_through"),
        );
        let rid = forward(ctx, &doc, &job_name, &user, &attrs)?;
        wait_remote(ctx, id, rid)?;
        set_job_state(ctx, id, J_COMPLETED, JobMessage::key("daemon.job.done"));
        return Ok(());
    }

    let npages = qpdf_npages(&doc)?;
    log(&t!(
        "daemon.log.job_summary",
        id = id,
        name = job_name.as_str(),
        pages = npages
    ));

    // pass 1: odd pages
    qpdf(&[&doc, "--pages", ".", "1-z:odd", "--", &p("odd.pdf")])?;
    set_job_state(
        ctx,
        id,
        J_PROCESSING,
        JobMessage::key("daemon.job.printing_odd"),
    );
    let rid = forward(
        ctx,
        &p("odd.pdf"),
        &format!("{job_name} [odd pages]"),
        &user,
        &attrs,
    )?;
    wait_remote(ctx, id, rid)?;

    if npages <= 1 {
        set_job_state(
            ctx,
            id,
            J_COMPLETED,
            JobMessage::key("daemon.job.done_single_page"),
        );
        return Ok(());
    }

    // pass 2: even pages reversed. For an even page count the odd positions
    // of the reversed range are the even pages descending; for an odd count
    // the even positions.
    let sel = if npages % 2 == 0 {
        "z-1:odd"
    } else {
        "z-1:even"
    };
    qpdf(&[&doc, "--pages", ".", sel, "--", &p("even.pdf")])?;
    let mut even = p("even.pdf");
    if npages % 2 == 1 && ctx.cfg.blank != "none" {
        std::fs::write(p("blank.pdf"), BLANK_PDF).map_err(|e| e.to_string())?;
        let (a, ar, b, br) = if ctx.cfg.blank == "trailing" {
            (even.clone(), "1-z", p("blank.pdf"), "1")
        } else {
            (p("blank.pdf"), "1", even.clone(), "1-z")
        };
        qpdf(&[
            "--empty",
            "--pages",
            &a,
            ar,
            &b,
            br,
            "--",
            &p("even-padded.pdf"),
        ])?;
        even = p("even-padded.pdf");
    }
    if ctx.cfg.rotate_even == 180 {
        qpdf(&[&even, "--rotate=+180:1-z", "--", &p("even-rotated.pdf")])?;
        even = p("even-rotated.pdf");
    }

    // wait for the flip
    *ctx.flip.requested.lock().unwrap() = false;
    let auto = ctx.cfg.auto_continue;
    let deadline = (auto > 0).then(|| Instant::now() + Duration::from_secs(auto));
    let waiting = if auto > 0 {
        JobMessage::with("daemon.job.flip_auto", vec![("secs", auto.to_string())])
    } else if ctx.cfg.gui {
        JobMessage::key("daemon.job.flip_gui")
    } else {
        JobMessage::with("daemon.job.flip_http", vec![("port", ctx.port.to_string())])
    };
    let said = waiting.render(lang());
    set_job_state(ctx, id, J_STOPPED, waiting);
    log(&t!(
        "daemon.log.job_odd_done",
        id = id,
        message = said.as_str()
    ));
    // the guard kills a still-open dialog on every exit path
    let _dialog = if ctx.cfg.gui && auto == 0 {
        DialogGuard(spawn_flip_dialog(ctx, &job_name))
    } else {
        DialogGuard(None)
    };
    loop {
        if job_canceled(ctx, id) {
            return Ok(());
        }
        let mut req = ctx.flip.requested.lock().unwrap();
        if *req || deadline.is_some_and(|d| Instant::now() >= d) {
            *req = false;
            break;
        }
        let _ = ctx
            .flip
            .cv
            .wait_timeout(req, Duration::from_secs(1))
            .unwrap();
    }

    set_job_state(
        ctx,
        id,
        J_PROCESSING,
        JobMessage::key("daemon.job.waiting_settle"),
    );
    wait_for_printer(ctx);
    set_job_state(
        ctx,
        id,
        J_PROCESSING,
        JobMessage::key("daemon.job.printing_even"),
    );
    let rid = forward(
        ctx,
        &even,
        &format!("{job_name} [even pages]"),
        &user,
        &attrs,
    )?;
    wait_remote(ctx, id, rid)?;
    set_job_state(ctx, id, J_COMPLETED, JobMessage::key("daemon.job.done"));
    log(&t!("daemon.log.job_completed", id = id));
    Ok(())
}

struct Cleanup(std::path::PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Job attributes that must NOT be forwarded: either we applied them
/// ourselves or they would change the pagination underneath our split.
const NO_FORWARD: &[&str] = &[
    "sides",
    "copies",
    "page-ranges",
    "number-up",
    "number-up-layout",
    "page-border",
    "multiple-document-handling",
    "job-hold-until",
];

fn forward(ctx: &Ctx, file: &str, name: &str, user: &str, attrs: &[Attr]) -> Result<i32, String> {
    let data = std::fs::read(file).map_err(|e| e.to_string())?;
    let mut msg = Msg::new(ipp::OP_PRINT_JOB, 1);
    msg.groups.push(Group {
        tag: ipp::G_OPERATION,
        attrs: vec![
            Attr::str(ipp::T_CHARSET, "attributes-charset", "utf-8"),
            Attr::str(ipp::T_LANGUAGE, "attributes-natural-language", "en"),
            Attr::str(ipp::T_URI, "printer-uri", &ctx.cfg.printer.uri),
            Attr::str(ipp::T_NAME, "requesting-user-name", user),
            Attr::str(ipp::T_NAME, "job-name", name),
            Attr::str(ipp::T_MIME, "document-format", "application/pdf"),
        ],
    });
    let mut fwd: Vec<Attr> = attrs
        .iter()
        .filter(|a| !NO_FORWARD.contains(&a.name.as_str()))
        .cloned()
        .collect();
    fwd.push(Attr::str(ipp::T_KEYWORD, "sides", "one-sided"));
    msg.groups.push(Group {
        tag: ipp::G_JOB,
        attrs: fwd,
    });
    msg.data = data;
    // A printer that has just finished job 1 often needs a moment before it
    // takes job 2. Retry transport errors and server errors (0x04xx, the
    // transient class); anything else is our fault, so do not loop on it.
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2));
        }
        match ipp::request(&ctx.cfg.printer, &msg) {
            Ok(resp) if resp.op_status > 0x00ff => {
                last = t!(
                    "daemon.err.remote_rejected",
                    status = format!("{:04x}", resp.op_status)
                )
                .to_string();
                if resp.op_status >> 8 != 0x04 {
                    return Err(last);
                }
            }
            Ok(resp) => {
                return resp
                    .find("job-id")
                    .and_then(|a| a.first_int())
                    .ok_or_else(|| t!("daemon.err.remote_no_job_id").to_string());
            }
            Err(e) => last = t!("daemon.err.remote_lost", err = e).to_string(),
        }
    }
    Err(last)
}

/// The state of the real printer (3 = idle), if it answers at all.
fn printer_state(ctx: &Ctx) -> Option<i32> {
    let mut q = Msg::new(ipp::OP_GET_PRINTER_ATTRS, 1);
    q.groups.push(Group {
        tag: ipp::G_OPERATION,
        attrs: vec![
            Attr::str(ipp::T_CHARSET, "attributes-charset", "utf-8"),
            Attr::str(ipp::T_LANGUAGE, "attributes-natural-language", "en"),
            Attr::str(ipp::T_URI, "printer-uri", &ctx.cfg.printer.uri),
            Attr::str(ipp::T_KEYWORD, "requested-attributes", "printer-state"),
        ],
    });
    ipp::request(&ctx.cfg.printer, &q)
        .ok()?
        .find("printer-state")?
        .first_int()
}

/// Wait for the real printer to settle before feeding it the backs.
///
/// A USB printer that just finished the odd pages is still processing, or has
/// fallen asleep; sending the even pages right away is what made them vanish.
/// Wait until it reports idle, but never longer than `--settle`: if it cannot
/// be asked, try the backs anyway and let the retry in `forward` cope.
fn wait_for_printer(ctx: &Ctx) {
    if ctx.cfg.settle == 0 {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(ctx.cfg.settle);
    loop {
        if printer_state(ctx) == Some(3) {
            return;
        }
        if Instant::now() >= deadline {
            log(&t!("daemon.log.settle_timeout", secs = ctx.cfg.settle));
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn wait_remote(ctx: &Ctx, id: i32, remote_id: i32) -> Result<(), String> {
    let mut errors = 0;
    loop {
        if job_canceled(ctx, id) {
            // best effort: cancel the remote job too
            let mut c = Msg::new(ipp::OP_CANCEL_JOB, 1);
            c.groups.push(Group {
                tag: ipp::G_OPERATION,
                attrs: vec![
                    Attr::str(ipp::T_CHARSET, "attributes-charset", "utf-8"),
                    Attr::str(ipp::T_LANGUAGE, "attributes-natural-language", "en"),
                    Attr::str(ipp::T_URI, "printer-uri", &ctx.cfg.printer.uri),
                    Attr::int(ipp::T_INTEGER, "job-id", remote_id),
                ],
            });
            let _ = ipp::request(&ctx.cfg.printer, &c);
            return Ok(());
        }
        let mut q = Msg::new(ipp::OP_GET_JOB_ATTRS, 1);
        q.groups.push(Group {
            tag: ipp::G_OPERATION,
            attrs: vec![
                Attr::str(ipp::T_CHARSET, "attributes-charset", "utf-8"),
                Attr::str(ipp::T_LANGUAGE, "attributes-natural-language", "en"),
                Attr::str(ipp::T_URI, "printer-uri", &ctx.cfg.printer.uri),
                Attr::int(ipp::T_INTEGER, "job-id", remote_id),
                Attr::str(ipp::T_KEYWORD, "requested-attributes", "job-state"),
            ],
        });
        match ipp::request(&ctx.cfg.printer, &q) {
            Ok(resp) => {
                errors = 0;
                match resp.find("job-state").and_then(|a| a.first_int()) {
                    Some(s) if s == J_COMPLETED => return Ok(()),
                    Some(s) if s == J_CANCELED || s == J_ABORTED => {
                        return Err(t!(
                            "daemon.err.remote_state",
                            id = remote_id,
                            state = l10n::text(lang(), state_key(s))
                        )
                        .to_string());
                    }
                    _ => {}
                }
            }
            Err(e) => {
                errors += 1;
                if errors >= 5 {
                    return Err(t!("daemon.err.remote_lost", err = e).to_string());
                }
            }
        }
        std::thread::sleep(Duration::from_secs(ctx.cfg.poll));
    }
}
