//! The language support: the catalogs agree with each other and with the
//! source, and the binaries and the status page actually speak the language
//! that was asked for.
//!
//! The catalog tests need no daemon; the behavior tests run the real binaries.

use ipp_duplexd::ipp::{self, Attr, Group, Msg};
use ipp_duplexd::l10n;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// -------------------------------------------------------------- the catalogs

#[test]
fn every_key_exists_in_every_locale_with_the_same_placeholders() {
    let reference = l10n::keys(l10n::FALLBACK);
    assert!(
        reference.len() > 30,
        "the reference catalog looks empty: {} keys",
        reference.len()
    );
    for locale in l10n::codes() {
        if locale == l10n::FALLBACK {
            continue;
        }
        for key in &reference {
            assert!(
                l10n::has_key(locale, key),
                "{locale} is missing the key {key}"
            );
            let en = l10n::text(l10n::FALLBACK, key);
            let translated = l10n::text(locale, key);
            assert!(!translated.trim().is_empty(), "{locale}.{key} is empty");
            assert_eq!(
                l10n::placeholders(&translated),
                l10n::placeholders(&en),
                "{locale}.{key} does not use the same placeholders as English"
            );
        }
    }
}

#[test]
fn the_compiled_locales_are_the_known_catalog() {
    let mut known = l10n::codes();
    known.sort_unstable();
    assert_eq!(l10n::compiled_locales(), known);
}

#[test]
fn every_key_used_in_the_source_exists() {
    let used = key_literals();
    assert!(!used.is_empty(), "found no translation keys in the source");
    for key in &used {
        assert!(
            l10n::has_key(l10n::FALLBACK, key),
            "{key} is used in the source but is not in the catalog"
        );
    }
}

#[test]
fn no_key_is_unused() {
    let used = key_literals();
    for key in l10n::keys(l10n::FALLBACK) {
        if key.starts_with('_') {
            // rust-i18n's own bookkeeping (`_version`), not a message
            continue;
        }
        assert!(
            used.contains(&key),
            "{key} is in the catalog but nothing uses it"
        );
    }
}

/// Every string literal in the sources that looks like a catalog key.
fn key_literals() -> BTreeSet<String> {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![src.join("main.rs"), src.join("l10n.rs")];
    for entry in std::fs::read_dir(src.join("bin")).unwrap() {
        files.push(entry.unwrap().path());
    }
    let mut found = BTreeSet::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for line in text.lines() {
            // split on quotes: the odd pieces are the literal contents
            for (i, piece) in line.split('"').enumerate() {
                if i % 2 == 1 && looks_like_a_key(piece) {
                    found.insert(piece.to_string());
                }
            }
        }
    }
    found
}

fn looks_like_a_key(piece: &str) -> bool {
    matches!(piece.split('.').next(), Some("daemon") | Some("setup"))
        && piece
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_')
        && piece.matches('.').count() >= 1
}

// --------------------------------------------------------- the daemon binary

fn duplexd(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ipp-duplexd"));
    cmd.args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run ipp-duplexd")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn the_usage_text_speaks_the_requested_language() {
    let out = duplexd(&[], &[("IPP_DUPLEXD_LANG", "en")]);
    assert_eq!(out.status.code(), Some(2));
    let en = stderr(&out);
    assert!(en.contains("--printer is required"), "{en}");
    assert!(en.contains("usage: ipp-duplexd"), "{en}");

    let out = duplexd(&["--lang", "zh-CN"], &[("IPP_DUPLEXD_LANG", "en")]);
    assert_eq!(out.status.code(), Some(2));
    let zh = stderr(&out);
    assert!(zh.contains("必须指定 --printer"), "{zh}");
    assert!(zh.contains("用法："), "{zh}");
    assert!(!zh.contains("usage: ipp-duplexd"), "{zh}");
}

#[test]
fn the_environment_picks_the_language_too() {
    for tag in ["zh_CN.UTF-8", "zh-TW", "zh-Hant"] {
        let out = duplexd(&[], &[("IPP_DUPLEXD_LANG", tag)]);
        assert_eq!(out.status.code(), Some(2));
        assert!(
            stderr(&out).contains("必须指定 --printer"),
            "{tag} did not select Chinese: {}",
            stderr(&out)
        );
    }
    // IPP_DUPLEXD_LANG wins over the conventional variables
    let out = duplexd(&[], &[("IPP_DUPLEXD_LANG", "en"), ("LANG", "zh_CN.UTF-8")]);
    assert!(stderr(&out).contains("--printer is required"));
}

#[test]
fn an_unknown_language_is_rejected_with_the_list_of_known_ones() {
    let out = duplexd(&["--lang", "de-DE"], &[("IPP_DUPLEXD_LANG", "en")]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("unsupported language 'de-DE'"), "{err}");
    assert!(err.contains("zh-CN (简体中文)"), "{err}");
    assert!(err.contains("en (English)"), "{err}");
}

// ------------------------------------------------- the daemon over HTTP

struct Daemon {
    child: std::process::Child,
    log: PathBuf,
}

impl Daemon {
    /// What the daemon said on stderr so far.
    fn said(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.log);
    }
}

/// The listening port, read out of the start-up line without depending on the
/// language that line is written in: the address is the only part of it that is
/// not translated.
fn port_in_log(text: &str) -> Option<u16> {
    let mut ports = Vec::new();
    for (at, _) in text.match_indices("127.0.0.1:") {
        let rest = &text[at + "127.0.0.1:".len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        // skip the --printer fixture, which points at the discard port
        if let Ok(port) = digits.parse::<u16>() {
            if port >= 1024 && !ports.contains(&port) {
                ports.push(port);
            }
        }
    }
    match ports.as_slice() {
        [port] => Some(*port),
        _ => None,
    }
}

#[test]
fn the_port_is_found_in_either_language() {
    let en = "ipp-duplexd: listening on 127.0.0.1:41234 as printer 'x', \
              forwarding to ipp://127.0.0.1:9/print";
    assert_eq!(port_in_log(en), Some(41234));
    let zh = "ipp-duplexd: 已作为打印机 'x' 监听 127.0.0.1:41234，转发到 ipp://127.0.0.1:9/print";
    assert_eq!(port_in_log(zh), Some(41234));
    assert_eq!(port_in_log("ipp-duplexd: something else entirely"), None);
    assert_eq!(port_in_log("... 127.0.0.1:1 and 127.0.0.1:2 ..."), None);
}

/// Start ipp-duplexd on a kernel-assigned port; `extra` may change the language.
#[allow(clippy::zombie_processes)] // the returned Daemon owns the child
fn start_daemon_with(extra: &[&str]) -> (Daemon, u16) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let log = std::env::temp_dir().join(format!(
        "ipp-duplexd-i18n-{}-{}.log",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let file = std::fs::File::create(&log).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_ipp-duplexd"))
        .args([
            "--printer",
            "ipp://127.0.0.1:9/print",
            "--listen",
            "127.0.0.1:0",
            "--no-gui",
            "--poll",
            "1",
        ])
        .args(extra)
        .env("IPP_DUPLEXD_LANG", "en")
        .env("LANGUAGE", "")
        .env("LC_ALL", "en_US.UTF-8")
        .env("LANG", "en_US.UTF-8")
        .stderr(Stdio::from(file))
        .spawn()
        .expect("failed to start ipp-duplexd");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if let Some(port) = port_in_log(&text) {
            return (
                Daemon {
                    child,
                    log: log.clone(),
                },
                port,
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("ipp-duplexd never reported its port");
}

/// The English daemon (the request headers decide what the HTTP answers say).
fn start_daemon() -> (Daemon, u16) {
    start_daemon_with(&[])
}

/// A raw HTTP exchange, so the request headers can be chosen freely.
fn http(port: u16, request: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

fn get(port: u16, path: &str, accept_language: &str) -> String {
    http(
        port,
        &format!(
            "GET {path} HTTP/1.1\r\nHost: t\r\nAccept-Language: {accept_language}\r\nConnection: close\r\n\r\n"
        ),
    )
}

#[test]
fn the_status_page_follows_accept_language() {
    let (_daemon, port) = start_daemon();

    let zh = get(port, "/", "zh-CN,zh;q=0.9,en;q=0.5");
    assert!(zh.contains("翻好纸后请求 GET /flip 继续"), "{zh}");
    assert!(zh.contains("Content-Language: zh-CN"), "{zh}");
    assert!(!zh.contains("GET /flip to continue"), "{zh}");

    let en = get(port, "/", "en-US,en;q=0.9");
    assert!(en.contains("GET /flip to continue after flipping"), "{en}");
    assert!(en.contains("Content-Language: en"), "{en}");

    // a language we do not have falls back to English, not to nothing
    let de = get(port, "/", "de-DE");
    assert!(de.contains("GET /flip to continue after flipping"), "{de}");
    assert!(de.contains("Content-Language: en"), "{de}");
}

#[test]
fn the_flip_endpoint_and_method_errors_follow_accept_language() {
    let (_daemon, port) = start_daemon();

    let zh = get(port, "/flip", "zh-CN");
    assert!(zh.contains("当前没有作业在等待翻纸"), "{zh}");
    assert!(zh.contains("Content-Language: zh-CN"), "{zh}");

    let zh = http(
        port,
        "PUT /nope HTTP/1.1\r\nHost: t\r\nAccept-Language: zh-CN\r\nConnection: close\r\n\r\n",
    );
    assert!(zh.contains("不支持该请求方法"), "{zh}");

    let en = get(port, "/flip", "en");
    assert!(en.contains("no job is waiting for a flip"), "{en}");
}

// ------------------------------------------------------- the IPP responses

fn printer_attributes(port: u16) -> Msg {
    let uri = format!("ipp://127.0.0.1:{port}/ipp/print");
    let target = ipp::parse_uri(&uri).expect("bad test printer URI");
    let mut m = Msg::new(ipp::OP_GET_PRINTER_ATTRS, 1);
    m.groups.push(Group {
        tag: ipp::G_OPERATION,
        attrs: vec![
            Attr::str(ipp::T_CHARSET, "attributes-charset", "utf-8"),
            Attr::str(ipp::T_LANGUAGE, "attributes-natural-language", "en"),
            Attr::str(ipp::T_URI, "printer-uri", &uri),
            Attr::str(ipp::T_KEYWORD, "requested-attributes", "all"),
        ],
    });
    ipp::request(&target, &m).expect("the daemon did not answer Get-Printer-Attributes")
}

fn attr_text(msg: &Msg, name: &str) -> Option<String> {
    msg.find(name)
        .and_then(|a| a.first_str())
        .map(str::to_string)
}

#[test]
fn ipp_responses_declare_the_language_of_their_text() {
    // The fixture printer never answers, so the state message is our text.
    let (daemon, port) = start_daemon_with(&["--lang", "zh-CN"]);
    let said = daemon.said();
    assert!(
        said.contains(&format!("127.0.0.1:{port}")),
        "the start-up line lost the address: {said}"
    );
    let zh = printer_attributes(port);
    assert_eq!(
        attr_text(&zh, "attributes-natural-language").as_deref(),
        Some("zh-CN")
    );
    assert_eq!(
        attr_text(&zh, "natural-language-configured").as_deref(),
        Some("zh-CN")
    );
    assert_eq!(
        attr_text(&zh, "printer-state-message").as_deref(),
        Some("连不上真实打印机")
    );
    let supported: Vec<String> = zh
        .find("generated-natural-language-supported")
        .map(|a| {
            a.values
                .iter()
                .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(supported.iter().any(|c| c == "zh-CN"), "{supported:?}");
    assert!(supported.iter().any(|c| c == "en"), "{supported:?}");

    // The default daemon says the same in English and declares English.
    let (_english_daemon, english_port) = start_daemon();
    let en = printer_attributes(english_port);
    assert_eq!(
        attr_text(&en, "attributes-natural-language").as_deref(),
        Some("en")
    );
    assert_eq!(
        attr_text(&en, "printer-state-message").as_deref(),
        Some("real printer is unreachable")
    );
}

// ---------------------------------------------------------- the setup tool

fn setup(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ipp-duplexd-setup"))
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::temp_dir())
        .env("IPP_DUPLEXD_LANG", "en")
        .output()
        .expect("failed to run ipp-duplexd-setup")
}

#[test]
fn the_setup_wizard_speaks_the_requested_language() {
    let out = setup(&["--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("set up the manual duplex virtual printer"),
        "{stdout}"
    );
    assert!(stdout.contains("--lang CODE"), "{stdout}");

    let out = setup(&["--lang", "zh-CN", "--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.contains("配置手动双面虚拟打印机"), "{stdout}");
    assert!(stdout.contains("--lang CODE"), "{stdout}");

    let out = setup(&["--lang", "de-DE"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("unsupported language 'de-DE'"), "{err}");
    assert!(err.contains("zh-CN (简体中文)"), "{err}");
}
