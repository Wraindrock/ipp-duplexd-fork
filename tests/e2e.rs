//! End-to-end tests for ipp-duplexd-fork: a mock IPP printer stands in for the real
//! printer and the tests play CUPS as the IPP client. Requires qpdf.
//! Every test gets its own daemon, mock, and ports, so they run in parallel.
//!
//! Pages of the generated test PDFs have distinct widths (601, 602, ...),
//! so the page order of the passes the mock receives is asserted exactly.
//!
//! Run with: cargo test -- --nocapture

use ipp_duplexd_fork::ipp::{self, Attr, Group, Msg};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ------------------------------------------------------------ test fixtures

/// n-page PDF; page k has MediaBox width 600+k so it is identifiable.
fn make_pdf(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = Vec::new();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objs.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {n} >>",
        kids.join(" ")
    ));
    for i in 0..n {
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} 792] /Resources << >> >>",
            601 + i
        ));
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    out
}

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> TestDir {
        let d = std::env::temp_dir().join(format!(
            "ipp-duplexd-fork-e2e-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        TestDir(d)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        // E2E_KEEP=1 keeps temp dirs (daemon logs, work files) for debugging
        if std::env::var_os("E2E_KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// (width, rotation) per page, in page order, via per-page qpdf extraction.
fn page_geometry(doc: &[u8], dir: &Path) -> Vec<(i32, i32)> {
    let src = dir.join("geom-src.pdf");
    let one = dir.join("geom-page.pdf");
    std::fs::write(&src, doc).unwrap();
    let npages: usize = {
        let out = Command::new("qpdf")
            .args(["--warning-exit-0", "--show-npages"])
            .arg(&src)
            .output()
            .expect("qpdf is required for the tests");
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
    };
    (1..=npages)
        .map(|k| {
            let ok = Command::new("qpdf")
                .args(["--warning-exit-0"])
                .arg(&src)
                .args(["--pages", ".", &k.to_string(), "--"])
                .arg(&one)
                .status()
                .unwrap()
                .success();
            assert!(ok, "qpdf page extraction failed");
            let text = String::from_utf8_lossy(&std::fs::read(&one).unwrap()).into_owned();
            (
                num_after(&text, "/MediaBox", 2).expect("page has no MediaBox"),
                num_after(&text, "/Rotate", 0).unwrap_or(0),
            )
        })
        .collect()
}

/// nth (0-based) integer following the first occurrence of `key`.
fn num_after(text: &str, key: &str, nth: usize) -> Option<i32> {
    let after = &text[text.find(key)? + key.len()..];
    after
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .filter(|s| s.parse::<i32>().is_ok())
        .nth(nth)
        .and_then(|s| s.parse().ok())
}

fn widths(doc: &[u8], dir: &Path) -> Vec<i32> {
    page_geometry(doc, dir).iter().map(|(w, _)| *w).collect()
}

// ------------------------------------------------------- mock real printer

#[derive(Clone, Debug)]
struct ReceivedJob {
    name: String,
    sides: Option<String>,
    quality: Option<i32>,
    doc: Vec<u8>,
}

/// Knobs for the awkward-but-real printer behaviour the daemon has to cope
/// with: a device that is still busy after job 1, and one that refuses the
/// first attempt at job 2 with a transient server error.
#[derive(Default)]
struct MockKnobs {
    /// answers left that report printer-state=processing (4) before idle
    busy: usize,
    /// reject the first attempt at the second print job with 0x0405 once
    reject_next: bool,
    /// print jobs received (including a rejected one)
    prints: usize,
    /// busy answers still pending when the backs arrived
    busy_at_backs: Option<usize>,
}

fn start_mock() -> (u16, Arc<Mutex<Vec<ReceivedJob>>>) {
    start_mock_with(Arc::new(Mutex::new(MockKnobs::default())))
}

fn start_mock_with(knobs: Arc<Mutex<MockKnobs>>) -> (u16, Arc<Mutex<Vec<ReceivedJob>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = Arc::new(Mutex::new(Vec::new()));
    let recs = received.clone();
    std::thread::spawn(move || {
        let mut next_id = 1i32;
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let _ = serve_mock_conn(stream, &recs, &mut next_id, &knobs);
        }
    });
    (port, received)
}

fn serve_mock_conn(
    stream: TcpStream,
    recs: &Arc<Mutex<Vec<ReceivedJob>>>,
    next_id: &mut i32,
    knobs: &Arc<Mutex<MockKnobs>>,
) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        reader.read_line(&mut h)?;
        if h == "\r\n" || h == "\n" || h.is_empty() {
            break;
        }
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    let req = ipp::parse(&body).expect("mock received unparseable ipp");

    let mut resp = Msg::response(ipp::OK, req.request_id);
    match req.op_status {
        ipp::OP_GET_PRINTER_ATTRS => {
            resp.groups.push(Group {
                tag: ipp::G_PRINTER,
                attrs: vec![
                    Attr::str(ipp::T_TEXT, "printer-make-and-model", "Brother HL-1234"),
                    Attr::strs(
                        ipp::T_KEYWORD,
                        "media-supported",
                        &["iso_a4_210x297mm", "na_letter_8.5x11in", "iso_a5_148x210mm"],
                    ),
                    Attr::strs(ipp::T_KEYWORD, "sides-supported", &["one-sided"]),
                    Attr::str(ipp::T_NAME, "printer-name", "real-laser"),
                    Attr::int(ipp::T_ENUM, "printer-state", printer_state(knobs)),
                    Attr::str(ipp::T_KEYWORD, "printer-state-reasons", "none"),
                    Attr::boolean("printer-is-accepting-jobs", true),
                ],
            });
        }
        ipp::OP_PRINT_JOB => {
            let id = *next_id;
            *next_id += 1;
            let accepted = {
                let mut k = knobs.lock().unwrap();
                k.prints += 1;
                if k.prints == 2 && k.reject_next {
                    k.reject_next = false;
                    false
                } else {
                    if k.prints == 2 {
                        k.busy_at_backs = Some(k.busy);
                    }
                    true
                }
            };
            if !accepted {
                // server-error-busy: the class the daemon must retry
                resp.op_status = 0x0405;
                let body = resp.encode();
                write!(
                    writer,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )?;
                writer.write_all(&body)?;
                return Ok(());
            }
            recs.lock().unwrap().push(ReceivedJob {
                name: req
                    .find("job-name")
                    .and_then(|a| a.first_str())
                    .unwrap_or("?")
                    .into(),
                sides: req
                    .find("sides")
                    .and_then(|a| a.first_str())
                    .map(String::from),
                quality: req.find("print-quality").and_then(|a| a.first_int()),
                doc: req.data.clone(),
            });
            resp.groups.push(Group {
                tag: ipp::G_JOB,
                attrs: vec![
                    Attr::int(ipp::T_INTEGER, "job-id", id),
                    Attr::int(ipp::T_ENUM, "job-state", 5),
                ],
            });
        }
        ipp::OP_GET_JOB_ATTRS => {
            // every mock job completes instantly
            resp.groups.push(Group {
                tag: ipp::G_JOB,
                attrs: vec![Attr::int(ipp::T_ENUM, "job-state", 9)],
            });
        }
        ipp::OP_CANCEL_JOB => {}
        _ => resp.op_status = ipp::ERR_OP_NOT_SUPPORTED,
    }
    let body = resp.encode();
    write!(
        writer,
        "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    writer.write_all(&body)?;
    Ok(())
}

// ------------------------------------------------------------- ipp client

/// 3 = idle, 4 = processing while the knobs say the printer is still busy.
fn printer_state(knobs: &Arc<Mutex<MockKnobs>>) -> i32 {
    let mut k = knobs.lock().unwrap();
    if k.busy > 0 {
        k.busy -= 1;
        4
    } else {
        3
    }
}

fn ipp_call(port: u16, msg: &Msg) -> Msg {
    let uri = ipp::parse_uri(&format!("ipp://127.0.0.1:{port}/ipp/print")).unwrap();
    ipp::request(&uri, msg).expect("ipp request failed")
}

fn base_op_attrs(port: u16) -> Vec<Attr> {
    vec![
        Attr::str(ipp::T_CHARSET, "attributes-charset", "utf-8"),
        Attr::str(ipp::T_LANGUAGE, "attributes-natural-language", "en"),
        Attr::str(
            ipp::T_URI,
            "printer-uri",
            &format!("ipp://127.0.0.1:{port}/ipp/print"),
        ),
    ]
}

fn get_printer_attrs(port: u16) -> Msg {
    let mut m = Msg::new(ipp::OP_GET_PRINTER_ATTRS, 1);
    m.groups.push(Group {
        tag: ipp::G_OPERATION,
        attrs: base_op_attrs(port),
    });
    ipp_call(port, &m)
}

fn print_job(port: u16, doc: &[u8], name: &str, job_attrs: Vec<Attr>) -> i32 {
    let mut m = Msg::new(ipp::OP_PRINT_JOB, 2);
    let mut op = base_op_attrs(port);
    op.push(Attr::str(ipp::T_NAME, "requesting-user-name", "tester"));
    op.push(Attr::str(ipp::T_NAME, "job-name", name));
    op.push(Attr::str(ipp::T_MIME, "document-format", "application/pdf"));
    m.groups.push(Group {
        tag: ipp::G_OPERATION,
        attrs: op,
    });
    m.groups.push(Group {
        tag: ipp::G_JOB,
        attrs: job_attrs,
    });
    m.data = doc.to_vec();
    let resp = ipp_call(port, &m);
    assert_eq!(resp.op_status, ipp::OK, "Print-Job rejected");
    resp.find("job-id")
        .and_then(|a| a.first_int())
        .expect("no job-id")
}

fn wait_job_state(port: u16, id: i32, want: &[i32], timeout: Duration) -> i32 {
    let deadline = Instant::now() + timeout;
    let mut last = -1;
    while Instant::now() < deadline {
        let mut m = Msg::new(ipp::OP_GET_JOB_ATTRS, 3);
        let mut op = base_op_attrs(port);
        op.push(Attr::int(ipp::T_INTEGER, "job-id", id));
        m.groups.push(Group {
            tag: ipp::G_OPERATION,
            attrs: op,
        });
        last = ipp_call(port, &m)
            .find("job-state")
            .and_then(|a| a.first_int())
            .unwrap_or(-1);
        if want.contains(&last) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("job {id} never reached {want:?}; last state {last}");
}

fn http_get(port: u16, path: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

// ---------------------------------------------------------- daemon control

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts ipp-duplexd-fork on 127.0.0.1:0 (kernel-assigned, so parallel tests can
/// never collide) and reads the actual port back from its log line.
fn start_daemon(
    mock_port: u16,
    extra: &[&str],
    env_path: Option<&str>,
    log: &Path,
) -> (Daemon, u16) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ipp-duplexd-fork"));
    cmd.args([
        "--printer",
        &format!("ipp://127.0.0.1:{mock_port}/print"),
        "--listen",
        "127.0.0.1:0",
        "--poll",
        "1",
    ])
    .args(extra)
    // the assertions below are about the English protocol text; do not let the
    // developer's locale leak into the daemon under test
    .env("IPP_DUPLEXD_FORK_LANG", "en")
    .stderr(Stdio::from(std::fs::File::create(log).unwrap()));
    if let Some(p) = env_path {
        cmd.env("PATH", p);
    }
    let daemon = Daemon(cmd.spawn().expect("failed to start ipp-duplexd-fork"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        if let Some(rest) = text.split("listening on 127.0.0.1:").nth(1) {
            let port: u16 = rest
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .expect("bad port in ipp-duplexd-fork log");
            return (daemon, port);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("ipp-duplexd-fork did not report its listen port; log: {log:?}");
}

fn attr_strs(msg: &Msg, name: &str) -> Vec<String> {
    msg.find(name)
        .map(|a| {
            a.values
                .iter()
                .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

// ------------------------------------------------------------------- tests

/// One fully wired instance: temp dir, mock printer, ipp-duplexd-fork on fresh ports.
struct Setup {
    dir: TestDir,
    received: Arc<Mutex<Vec<ReceivedJob>>>,
    port: u16,
    _daemon: Daemon,
}

fn setup_with_knobs(name: &str, extra: &[&str], knobs: Arc<Mutex<MockKnobs>>) -> Setup {
    let dir = TestDir::new(name);
    let (mock_port, received) = start_mock_with(knobs);
    let (daemon, port) = start_daemon(mock_port, extra, None, &dir.path("ipp-duplexd-fork.log"));
    Setup {
        dir,
        received,
        port,
        _daemon: daemon,
    }
}

fn setup(name: &str, extra: &[&str], env_path: Option<&str>) -> Setup {
    let dir = TestDir::new(name);
    let (mock_port, received) = start_mock();
    let (daemon, port) = start_daemon(
        mock_port,
        extra,
        env_path,
        &dir.path("ipp-duplexd-fork.log"),
    );
    Setup {
        dir,
        received,
        port,
        _daemon: daemon,
    }
}

#[test]
fn printer_attributes_proxied_and_mirrored() {
    let t = setup("attrs", &["--no-gui"], None);
    let attrs = get_printer_attrs(t.port);
    let mm = attrs
        .find("printer-make-and-model")
        .and_then(|a| a.first_str())
        .unwrap();
    assert!(
        mm.contains("Brother HL-1234") && mm.contains("manual duplex"),
        "{mm}"
    );
    assert!(attr_strs(&attrs, "media-supported").contains(&"iso_a5_148x210mm".to_string()));
    assert!(attr_strs(&attrs, "sides-supported").contains(&"two-sided-long-edge".to_string()));
    let uri = attrs
        .find("printer-uri-supported")
        .and_then(|a| a.first_str())
        .unwrap();
    assert!(uri.contains(&t.port.to_string()), "{uri}");
    assert_eq!(
        attrs.find("printer-state").and_then(|a| a.first_int()),
        Some(3),
        "mirrored idle"
    );
    assert_eq!(
        attrs.find("printer-is-accepting-jobs").unwrap().values[0].1,
        vec![1u8]
    );
}

#[test]
fn duplex_splits_pads_and_rotates() {
    // the padding and the rotation are options now, not defaults
    let t = setup(
        "duplex",
        &["--no-gui", "--blank", "leading", "--rotate-even", "180"],
        None,
    );
    let jid = print_job(
        t.port,
        &make_pdf(5),
        "doc5",
        vec![
            Attr::str(ipp::T_KEYWORD, "sides", "two-sided-long-edge"),
            Attr::int(ipp::T_ENUM, "print-quality", 5),
        ],
    );
    wait_job_state(t.port, jid, &[6], Duration::from_secs(15)); // stopped for flip
    {
        let r = t.received.lock().unwrap();
        assert_eq!(widths(&r[0].doc, &t.dir.0), vec![601, 603, 605], "odd pass");
        assert_eq!(r[0].name, "doc5 [odd pages]");
        assert_eq!(r[0].quality, Some(5), "print-quality forwarded");
        assert_eq!(r[0].sides.as_deref(), Some("one-sided"));
    }
    // while stopped, the printer state must say media-needed
    let attrs = get_printer_attrs(t.port);
    assert_eq!(
        attrs.find("printer-state").and_then(|a| a.first_int()),
        Some(5)
    );
    assert!(attr_strs(&attrs, "printer-state-reasons").contains(&"media-needed".to_string()));

    assert!(http_get(t.port, "/flip").contains("printing the backs"));
    wait_job_state(t.port, jid, &[9], Duration::from_secs(15));
    let r = t.received.lock().unwrap();
    // leading blank (width 612), then evens reversed; all rotated 180
    let geom = page_geometry(&r[1].doc, &t.dir.0);
    assert_eq!(
        geom.iter().map(|(w, _)| *w).collect::<Vec<_>>(),
        vec![612, 604, 602]
    );
    assert!(
        geom.iter().all(|(_, rot)| *rot == 180),
        "even pass rotated: {geom:?}"
    );
    let odd_geom = page_geometry(&r[0].doc, &t.dir.0);
    assert!(
        odd_geom.iter().all(|(_, rot)| *rot == 0),
        "odd pass unrotated"
    );
}

#[test]
fn one_sided_passes_through() {
    let t = setup("simplex", &["--no-gui"], None);
    let jid = print_job(
        t.port,
        &make_pdf(5),
        "doc5-simplex",
        vec![Attr::str(ipp::T_KEYWORD, "sides", "one-sided")],
    );
    wait_job_state(t.port, jid, &[9], Duration::from_secs(15));
    let r = t.received.lock().unwrap();
    assert_eq!(r.len(), 1, "exactly one pass");
    assert_eq!(widths(&r[0].doc, &t.dir.0), vec![601, 602, 603, 604, 605]);
}

#[test]
fn copies_and_page_ranges_apply_before_split() {
    let t = setup("copies", &["--no-gui"], None);
    // copies=2 and page-ranges 1-3 -> pages 1,2,3,1,2,3 before the split
    let mut range = 1i32.to_be_bytes().to_vec();
    range.extend_from_slice(&3i32.to_be_bytes());
    let jid = print_job(
        t.port,
        &make_pdf(6),
        "doc6",
        vec![
            Attr::int(ipp::T_INTEGER, "copies", 2),
            Attr {
                name: "page-ranges".into(),
                values: vec![(0x33, range)],
            },
        ],
    );
    wait_job_state(t.port, jid, &[6], Duration::from_secs(15));
    http_get(t.port, "/flip");
    wait_job_state(t.port, jid, &[9], Duration::from_secs(15));
    let r = t.received.lock().unwrap();
    assert_eq!(
        widths(&r[0].doc, &t.dir.0),
        vec![601, 603, 602],
        "odd of 1,2,3,1,2,3"
    );
    assert_eq!(
        widths(&r[1].doc, &t.dir.0),
        vec![603, 601, 602],
        "reversed evens"
    );
}

#[test]
fn gui_dialog_continues_job() {
    // a stub zenity "clicks OK", so the job finishes with no /flip call
    let dir = TestDir::new("gui");
    let bin = dir.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("zenity"), "#!/bin/sh\nsleep 0.7\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("zenity"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let t = setup(
        "gui-daemon",
        &["--blank", "leading", "--rotate-even", "180"],
        Some(&path),
    );
    let jid = print_job(
        t.port,
        &make_pdf(5),
        "doc5-gui",
        vec![Attr::str(ipp::T_KEYWORD, "sides", "two-sided-long-edge")],
    );
    wait_job_state(t.port, jid, &[9], Duration::from_secs(25));
    {
        let r = t.received.lock().unwrap();
        assert_eq!(widths(&r[0].doc, &t.dir.0), vec![601, 603, 605]);
        assert_eq!(widths(&r[1].doc, &t.dir.0), vec![612, 604, 602]);
    }
    let log = std::fs::read_to_string(t.dir.path("ipp-duplexd-fork.log")).unwrap();
    assert!(log.contains("flip dialog opened via zenity"), "{log}");
}

#[test]
fn offline_printer_is_mirrored() {
    // unreachable real printer -> stopped + offline-report, but still spooling.
    // Port 1 needs root to bind, so nothing can ever listen there.
    let dir = TestDir::new("offline");
    let (_d, port) = start_daemon(1, &["--no-gui"], None, &dir.path("ipp-duplexd-fork.log"));
    let attrs = get_printer_attrs(port);
    assert_eq!(
        attrs.find("printer-state").and_then(|a| a.first_int()),
        Some(5)
    );
    assert!(attr_strs(&attrs, "printer-state-reasons").contains(&"offline-report".to_string()));
    assert_eq!(
        attrs.find("printer-is-accepting-jobs").unwrap().values[0].1,
        vec![1u8],
        "still spooling"
    );
}

/// Field report: after the odd pages the printer is still processing (or has
/// gone to sleep), so the backs sent immediately are dropped. The daemon must
/// wait for printer-state=idle before it sends job 2.
#[test]
fn the_backs_wait_for_the_printer_to_settle() {
    let knobs = Arc::new(Mutex::new(MockKnobs {
        busy: 6,
        ..Default::default()
    }));
    let t = setup_with_knobs(
        "settle",
        &["--no-gui", "--settle", "30", "--poll", "1"],
        knobs.clone(),
    );
    let jid = print_job(
        t.port,
        &make_pdf(4),
        "doc4-settle",
        vec![Attr::str(ipp::T_KEYWORD, "sides", "two-sided-long-edge")],
    );
    wait_job_state(t.port, jid, &[6], Duration::from_secs(15));
    assert!(http_get(t.port, "/flip").contains("printing the backs"));
    wait_job_state(t.port, jid, &[9], Duration::from_secs(30));

    let k = knobs.lock().unwrap();
    assert_eq!(k.prints, 2, "one job per side");
    assert_eq!(
        k.busy_at_backs,
        Some(0),
        "the backs were sent while the printer still reported processing"
    );
    drop(k);
    assert_eq!(t.received.lock().unwrap().len(), 2);
}

/// The second job can also bounce once with a transient server error while
/// the device wakes up; that must be retried, not turned into a failed job.
#[test]
fn a_busy_printer_still_gets_the_backs() {
    let knobs = Arc::new(Mutex::new(MockKnobs {
        reject_next: true,
        ..Default::default()
    }));
    let t = setup_with_knobs(
        "busy",
        &["--no-gui", "--settle", "0", "--poll", "1"],
        knobs.clone(),
    );
    let jid = print_job(
        t.port,
        &make_pdf(4),
        "doc4-busy",
        vec![Attr::str(ipp::T_KEYWORD, "sides", "two-sided-long-edge")],
    );
    wait_job_state(t.port, jid, &[6], Duration::from_secs(15));
    assert!(http_get(t.port, "/flip").contains("printing the backs"));
    wait_job_state(t.port, jid, &[9], Duration::from_secs(30));

    assert_eq!(
        knobs.lock().unwrap().prints,
        3,
        "the rejected attempt must be retried, not dropped"
    );
    let r = t.received.lock().unwrap();
    assert_eq!(r.len(), 2, "odd + even documents actually printed");
    assert_eq!(r[1].name, "doc4-busy [even pages]");
}

// ------------------------------------------------------ several printers

/// Start the daemon from a configuration file instead of the flags.
fn start_daemon_from_config(config: &Path, log: &Path) -> Daemon {
    let cmd = Command::new(env!("CARGO_BIN_EXE_ipp-duplexd-fork"));
    let mut cmd = cmd;
    cmd.args(["--config", config.to_str().unwrap()])
        .env("IPP_DUPLEXD_FORK_LANG", "en")
        .stderr(Stdio::from(std::fs::File::create(log).unwrap()));
    Daemon(cmd.spawn().expect("failed to start ipp-duplexd-fork"))
}

/// The port the daemon reported for one virtual printer.
fn wait_for_printer(log: &Path, name: &str, timeout: Duration) -> u16 {
    let deadline = Instant::now() + timeout;
    let marker = format!("as printer '{name}'");
    while Instant::now() < deadline {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        for line in text.lines() {
            if line.contains(&marker) {
                if let Some(rest) = line.split("listening on 127.0.0.1:").nth(1) {
                    let port: u16 = rest
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect::<String>()
                        .parse()
                        .expect("bad port in ipp-duplexd-fork log");
                    return port;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("printer {name} never started; log: {log:?}");
}

fn write_config(path: &Path, printers: &[(&str, u16)]) {
    let mut text = String::new();
    for (name, mock) in printers {
        text.push_str(&format!(
            "[[printer]]\nname = \"{name}\"\nlisten = \"127.0.0.1:0\"\nreal = \"ipp://127.0.0.1:{mock}/print\"\npoll = 1\n\n"
        ));
    }
    std::fs::write(path, text).unwrap();
}

fn reload(port: u16) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "POST /reload HTTP/1.1\r\nHost: t\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn one_daemon_serves_two_virtual_printers() {
    let dir = TestDir::new("two-printers");
    let (mock_a, received_a) = start_mock();
    let (mock_b, received_b) = start_mock();
    let config = dir.path("config.toml");
    write_config(&config, &[("alpha", mock_a), ("beta", mock_b)]);
    let log = dir.path("daemon.log");
    let _daemon = start_daemon_from_config(&config, &log);

    let alpha = wait_for_printer(&log, "alpha", Duration::from_secs(5));
    let beta = wait_for_printer(&log, "beta", Duration::from_secs(5));
    assert_ne!(alpha, beta, "each printer needs its own port");

    // each virtual printer shows its own name and its own real printer
    let page_a = http_get(alpha, "/");
    assert!(page_a.contains("'alpha'"), "page A: {page_a}");
    assert!(page_a.contains(&format!("127.0.0.1:{mock_a}/print")));
    let page_b = http_get(beta, "/");
    assert!(page_b.contains("'beta'"), "page B: {page_b}");
    assert!(page_b.contains(&format!("127.0.0.1:{mock_b}/print")));

    // a job on one printer never shows up on the other, and the flip gate is
    // per printer too
    let id = print_job(alpha, &make_pdf(3), "alpha-doc", Vec::new());
    wait_job_state(alpha, id, &[6], Duration::from_secs(10));
    assert!(!http_get(beta, "/").contains("alpha-doc"));
    assert!(http_get(beta, "/flip").contains("no job is waiting"));
    assert!(http_get(alpha, "/flip").contains("ok, printing the backs"));
    wait_job_state(alpha, id, &[9], Duration::from_secs(10));
    assert_eq!(received_a.lock().unwrap().len(), 2, "odd then even");
    assert!(received_b.lock().unwrap().is_empty());
}

#[test]
fn an_empty_configuration_keeps_the_daemon_alive() {
    // Removing the last printer leaves a file with no [[printer]] at all. That
    // is a valid state, and the daemon must sit there waiting for a reload
    // instead of exiting into systemd's restart loop.
    let dir = TestDir::new("empty-config");
    let config = dir.path("config.toml");
    std::fs::write(&config, "").unwrap();
    let log = dir.path("daemon.log");

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_ipp-duplexd-fork"))
        .args(["--config", config.to_str().unwrap()])
        .env("IPP_DUPLEXD_FORK_LANG", "en")
        .stderr(Stdio::from(std::fs::File::create(&log).unwrap()))
        .spawn()
        .expect("failed to start ipp-duplexd-fork");

    std::thread::sleep(Duration::from_millis(700));
    let status = daemon.try_wait().expect("try_wait");
    let _ = daemon.kill();
    let _ = daemon.wait();
    let said = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        status.is_none(),
        "the daemon exited with {status:?} on an empty config; it said: {said}"
    );
}

#[test]
fn a_running_daemon_adopts_a_configuration_file_written_later() {
    // The window writes config.toml, but a daemon started from the legacy
    // flags had no file when it started: reload must notice the file and
    // serve it, or the manager would save settings nothing ever reads.
    let dir = TestDir::new("adopt-config");
    let (mock_legacy, _received_legacy) = start_mock();
    let (mock_window, _received_window) = start_mock();
    let config = dir.path("config.toml");
    let log = dir.path("daemon.log");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ipp-duplexd-fork"));
    let _daemon = Daemon(
        cmd.args([
            "--printer",
            &format!("ipp://127.0.0.1:{mock_legacy}/print"),
            "--listen",
            "127.0.0.1:0",
            "--poll",
            "1",
        ])
        .env("IPP_DUPLEXD_FORK_LANG", "en")
        .env("IPP_DUPLEXD_FORK_CONFIG", config.to_str().unwrap())
        .stderr(Stdio::from(std::fs::File::create(&log).unwrap()))
        .spawn()
        .expect("failed to start ipp-duplexd-fork"),
    );

    let legacy = wait_for_printer(&log, "manual-duplex", Duration::from_secs(5));
    assert!(http_get(legacy, "/").contains("'manual-duplex'"));

    // the window saves a file while the daemon runs
    write_config(&config, &[("window-added", mock_window)]);
    assert!(reload(legacy).contains("reloaded"));

    let added = wait_for_printer(&log, "window-added", Duration::from_secs(5));
    assert!(http_get(added, "/").contains("'window-added'"));

    // the legacy printer is not in the file, so its listener is gone
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut closed = false;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", legacy)).is_err() {
            closed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(closed, "the legacy printer should stop listening");
}

#[test]
fn reload_picks_up_added_and_removed_printers() {
    let dir = TestDir::new("reload");
    let (mock_a, _received_a) = start_mock();
    let (mock_b, _received_b) = start_mock();
    let config = dir.path("config.toml");
    write_config(&config, &[("alpha", mock_a)]);
    let log = dir.path("daemon.log");
    let _daemon = start_daemon_from_config(&config, &log);
    let alpha = wait_for_printer(&log, "alpha", Duration::from_secs(5));

    // add a second printer and reload through the first one
    write_config(&config, &[("alpha", mock_a), ("beta", mock_b)]);
    assert!(reload(alpha).contains("reloaded"));
    let beta = wait_for_printer(&log, "beta", Duration::from_secs(5));
    assert!(http_get(beta, "/").contains("'beta'"));
    assert!(http_get(alpha, "/").contains("'alpha'"), "alpha stays");

    // remove the first printer and reload through the second one
    write_config(&config, &[("beta", mock_b)]);
    assert!(reload(beta).contains("reloaded"));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut closed = false;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", alpha)).is_err() {
            closed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(closed, "alpha should stop listening after the reload");
    assert!(http_get(beta, "/").contains("'beta'"), "beta keeps serving");
    assert!(std::fs::read_to_string(&log).unwrap().contains("reloaded"));
}
