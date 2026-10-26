//! Printer discovery: ask the local CUPS daemon which queues it knows about, so
//! the window can offer them instead of asking for a URI.
//!
//! CUPS speaks IPP on its own socket, so `CUPS-Get-Printers` (0x4002) answers
//! with one printer group per queue. Everything here is part of the default
//! build, so it is tested without a display against a fake CUPS server.

use crate::ipp;

/// Where CUPS listens unless it was told otherwise.
pub const CUPS_ADDR: &str = "127.0.0.1:631";

/// CUPS-Get-Printers: not in RFC 8011, but every CUPS answers it.
pub const OP_CUPS_GET_PRINTERS: u16 = 0x4002;

/// One queue the local CUPS daemon knows about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovered {
    /// The CUPS queue name, for example `office`.
    pub queue: String,
    /// What CUPS says the printer is: make-and-model, falling back to info.
    pub model: String,
    /// The IPP URI that reaches this queue through CUPS.
    pub uri: String,
    /// `printer-state`: 3 idle, 4 processing, 5 stopped.
    pub state: Option<i32>,
}

/// Ask the local CUPS daemon which printers it has.
pub fn cups_printers() -> Result<Vec<Discovered>, String> {
    cups_printers_at(CUPS_ADDR)
}

/// The same question, aimed at another address, so tests can answer it.
pub fn cups_printers_at(addr: &str) -> Result<Vec<Discovered>, String> {
    let target = ipp::parse_uri(&format!("ipp://{addr}/"))?;
    let mut msg = ipp::Msg::new(OP_CUPS_GET_PRINTERS, 1);
    msg.groups.push(ipp::Group {
        tag: ipp::G_OPERATION,
        attrs: vec![
            ipp::Attr::str(ipp::T_CHARSET, "attributes-charset", "utf-8"),
            ipp::Attr::str(ipp::T_LANGUAGE, "attributes-natural-language", "en"),
            ipp::Attr::strs(
                ipp::T_KEYWORD,
                "requested-attributes",
                &[
                    "printer-name",
                    "printer-info",
                    "printer-make-and-model",
                    "printer-state",
                ],
            ),
        ],
    });
    let response = ipp::request(&target, &msg)?;
    if response.op_status != ipp::OK {
        return Err(format!("CUPS answered 0x{:04x}", response.op_status));
    }
    Ok(printers_in(&response))
}

/// Read the printer groups out of a `CUPS-Get-Printers` response.
pub fn printers_in(msg: &ipp::Msg) -> Vec<Discovered> {
    msg.groups
        .iter()
        .filter(|group| group.tag == ipp::G_PRINTER)
        .filter_map(|group| {
            let queue = text_of(group, "printer-name")?;
            let model = text_of(group, "printer-make-and-model")
                .or_else(|| text_of(group, "printer-info"))
                .unwrap_or_else(|| queue.clone());
            let state = group
                .attrs
                .iter()
                .find(|attr| attr.name == "printer-state")
                .and_then(|attr| attr.first_int());
            Some(Discovered {
                uri: suggested_uri(&queue),
                queue,
                model,
                state,
            })
        })
        .collect()
}

fn text_of(group: &ipp::Group, name: &str) -> Option<String> {
    group
        .attrs
        .iter()
        .find(|attr| attr.name == name)
        .and_then(|attr| attr.first_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The URI a virtual printer should forward to, for a queue on this machine.
///
/// CUPS reports `printer-uri-supported` too, but that is often `localhost` or
/// the build host's name; going through `127.0.0.1` is what works from here.
pub fn suggested_uri(queue: &str) -> String {
    format!("ipp://{CUPS_ADDR}/printers/{queue}")
}

/// A virtual printer name for a queue: `<queue>-duplex`, made unique.
pub fn suggested_name(queue: &str, existing: &[String]) -> String {
    let base = format!("{}-duplex", sanitize(queue));
    if !existing.iter().any(|name| name.eq_ignore_ascii_case(&base)) {
        return base;
    }
    for counter in 2..1000 {
        let candidate = format!("{base}-{counter}");
        if !existing
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
    }
    format!("{base}-{}", std::process::id())
}

/// Lowercase letters, digits and single dashes; the rest becomes a dash.
fn sanitize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "printer".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    /// A listen-and-answer-once CUPS stand-in: bind, answer, hand back the
    /// request bytes so the test can check what was asked.
    fn fake_cups(answer: ipp::Msg) -> (String, thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake CUPS socket");
        let addr = listener.local_addr().expect("local address").to_string();
        let body = answer.encode();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept one request");
            let request = read_request(&mut stream);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).expect("status line");
            stream.write_all(&body).expect("body");
            request
        });
        (addr, handle)
    }

    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            match stream.read(&mut byte) {
                Ok(0) | Err(_) => return request,
                Ok(_) => request.push(byte[0]),
            }
        }
        let header = String::from_utf8_lossy(&request).to_ascii_lowercase();
        let length = header
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; length];
        if stream.read_exact(&mut body).is_ok() {
            request.extend_from_slice(&body);
        }
        request
    }

    fn printer(queue: &str, model: &str, state: i32) -> ipp::Group {
        ipp::Group {
            tag: ipp::G_PRINTER,
            attrs: vec![
                ipp::Attr::str(ipp::T_NAME, "printer-name", queue),
                ipp::Attr::str(ipp::T_TEXT, "printer-make-and-model", model),
                ipp::Attr::int(ipp::T_ENUM, "printer-state", state),
            ],
        }
    }

    #[test]
    fn cups_printers_become_virtual_printers() {
        let mut msg = ipp::Msg::response(ipp::OK, 1);
        msg.groups.push(printer("office", "HP DeskJet 2600", 3));
        msg.groups.push(printer("Kitchen", "", 4));
        let found = printers_in(&msg);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].queue, "office");
        assert_eq!(found[0].model, "HP DeskJet 2600");
        assert_eq!(found[0].uri, "ipp://127.0.0.1:631/printers/office");
        assert_eq!(found[0].state, Some(3));
        assert_eq!(found[1].uri, "ipp://127.0.0.1:631/printers/Kitchen");
        // Without a model, the queue name stands in so the row is never blank.
        assert_eq!(found[1].model, "Kitchen");
    }

    #[test]
    fn a_group_without_a_name_is_ignored() {
        let mut msg = ipp::Msg::response(ipp::OK, 1);
        msg.groups.push(ipp::Group {
            tag: ipp::G_PRINTER,
            attrs: vec![ipp::Attr::str(ipp::T_TEXT, "printer-info", "orphan")],
        });
        assert!(printers_in(&msg).is_empty());
    }

    #[test]
    fn the_request_asks_for_the_printers() {
        let mut answer = ipp::Msg::response(ipp::OK, 1);
        answer.groups.push(printer("office", "HP", 3));
        let (addr, server) = fake_cups(answer);
        let found = cups_printers_at(&addr).expect("CUPS answers");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].queue, "office");

        let request = server.join().expect("the fake server saw a request");
        let body_start = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("header end")
            + 4;
        let msg = ipp::parse(&request[body_start..]).expect("an IPP request");
        assert_eq!(msg.op_status, OP_CUPS_GET_PRINTERS);
        let requested = msg
            .find("requested-attributes")
            .expect("requested-attributes");
        assert!(requested
            .values
            .iter()
            .any(|(_, value)| value == b"printer-name"));
    }

    #[test]
    fn a_cups_that_is_not_there_is_an_error() {
        let err = cups_printers_at("127.0.0.1:9").expect_err("nothing listens on port 9");
        assert!(!err.is_empty());
    }

    #[test]
    fn names_are_sanitized_and_made_unique() {
        assert_eq!(suggested_name("office", &[]), "office-duplex");
        assert_eq!(suggested_name("Office Laser", &[]), "office-laser-duplex");
        assert_eq!(suggested_name("--weird--", &[]), "weird-duplex");
        assert_eq!(suggested_name("", &[]), "printer-duplex");
        assert_eq!(
            suggested_name("office", &["office-duplex".to_string()]),
            "office-duplex-2"
        );
        assert_eq!(
            suggested_name(
                "office",
                &["office-duplex".to_string(), "OFFICE-DUPLEX-2".to_string()]
            ),
            "office-duplex-3"
        );
    }
}
