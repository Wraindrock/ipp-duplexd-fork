//! The hplip trap: a CUPS queue that drives a USB printer through hplip while
//! `ipp-usb` is the one that actually holds the device.
//!
//! Both drive the same USB printer. `ipp-usb` claims the interfaces and
//! publishes an IPP endpoint; hplip's `hp` backend then cannot open the device,
//! retries every 30 seconds and holds the queue, so jobs never print and
//! nothing says why. The fix is not to remove hplip — other software may use it
//! — but to stop *this queue* from going through it.
//!
//! Everything here is a decision, not an action: no CUPS calls, no files. The
//! window asks for a verdict and runs `lpadmin` itself.

use crate::discover::Discovered;

/// The endpoint `ipp-usb` publishes, and what a switched queue points at.
pub const IPP_USB_URI: &str = "ipp://localhost:60000/ipp/print";

/// Does this `device-uri` go through hplip's USB backend?
///
/// Only USB: a `hp:/net/…` queue talks to a network printer over TCP and never
/// touches the device ipp-usb is holding, so it must not be reported.
pub fn is_hplip_usb(device_uri: &str) -> bool {
    device_uri.starts_with("hp:/usb/")
}

/// The local CUPS queue a `real` URI points at.
///
/// Only loopback CUPS queues are interesting: a printer reached directly over
/// IPP has no queue of ours in between, and a queue on another machine is not
/// ours to change.
pub fn queue_of_real(real: &str) -> Option<String> {
    let rest = real.split_once("://")?.1;
    let (authority, path) = rest.split_once('/')?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host.split(':').next()?;
    if !matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1" | "[::1]"
    ) {
        return None;
    }
    let queue = path.strip_prefix("printers/")?;
    if queue.is_empty() || queue.contains('/') || queue.contains('?') {
        return None;
    }
    Some(queue.to_string())
}

/// A printer whose queue and `ipp-usb` will fight over the same device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The CUPS queue that drives the printer through hplip.
    pub queue: String,
    /// Its `device-uri`, so it can be put back later.
    pub device_uri: String,
}

/// The conflict behind one printer's `real` address, if there is one.
///
/// `ipp_usb` means "ipp-usb is answering on its port", not "the package is
/// installed": only a serving ipp-usb can be the one holding the device. On a
/// machine where hplip owns the printer there is no conflict to report.
///
/// It is passed in (not looked up) so the caller decides when to pay for the
/// check and the tests decide the answer.
pub fn conflict_for(real: &str, queues: &[Discovered], ipp_usb: bool) -> Option<Conflict> {
    if !ipp_usb {
        return None;
    }
    let queue = queue_of_real(real)?;
    let found = queues.iter().find(|known| known.queue == queue)?;
    let device_uri = found.device_uri.as_deref()?;
    if !is_hplip_usb(device_uri) {
        return None;
    }
    Some(Conflict {
        queue,
        device_uri: device_uri.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(name: &str, device: Option<&str>) -> Discovered {
        Discovered {
            queue: name.to_string(),
            model: name.to_string(),
            uri: format!("ipp://127.0.0.1:631/printers/{name}"),
            state: Some(3),
            device_uri: device.map(str::to_string),
        }
    }

    #[test]
    fn only_usb_queues_are_hplip_traps() {
        assert!(is_hplip_usb(
            "hp:/usb/DeskJet_2600_series?serial=CN8BE790YY06MD"
        ));
        // network printers never touch the device ipp-usb holds
        assert!(!is_hplip_usb("hp:/net/OfficeJet_Pro?ip=192.168.1.5"));
        // and a queue already on IPP is fine
        assert!(!is_hplip_usb("ipp://localhost:60000/ipp/print"));
        assert!(!is_hplip_usb("ipp://127.0.0.1:631/printers/laser"));
        assert!(!is_hplip_usb(""));
    }

    #[test]
    fn the_real_uri_names_a_local_queue() {
        assert_eq!(
            queue_of_real("ipp://127.0.0.1:631/printers/DeskJet-2600").as_deref(),
            Some("DeskJet-2600")
        );
        assert_eq!(
            queue_of_real("http://localhost:631/printers/office").as_deref(),
            Some("office")
        );
        // another machine's queue is not ours to change
        assert_eq!(
            queue_of_real("ipp://192.168.1.50:631/printers/office"),
            None
        );
        // not a queue at all: a printer on the network, or ipp-usb itself
        assert_eq!(queue_of_real("ipp://192.168.1.50/ipp/print"), None);
        assert_eq!(queue_of_real("ipp://localhost:60000/ipp/print"), None);
        assert_eq!(queue_of_real("ipp://127.0.0.1:631/printers/"), None);
        assert_eq!(queue_of_real("ipp://127.0.0.1:631/printers/a/b"), None);
        assert_eq!(queue_of_real("nonsense"), None);
    }

    #[test]
    fn a_hplip_queue_behind_a_real_printer_is_a_conflict() {
        let queues = vec![
            queue(
                "DeskJet-2600",
                Some("hp:/usb/DeskJet_2600_series?serial=CN8BE790YY06MD"),
            ),
            queue("office", Some("ipp://localhost:60000/ipp/print")),
        ];
        let real = "ipp://127.0.0.1:631/printers/DeskJet-2600";
        let conflict = conflict_for(real, &queues, true).expect("a conflict");
        assert_eq!(conflict.queue, "DeskJet-2600");
        assert_eq!(
            conflict.device_uri,
            "hp:/usb/DeskJet_2600_series?serial=CN8BE790YY06MD"
        );

        // without ipp-usb there is nothing to fight over
        assert_eq!(conflict_for(real, &queues, false), None);
        // a queue already on IPP is not a conflict
        assert_eq!(
            conflict_for("ipp://127.0.0.1:631/printers/office", &queues, true),
            None
        );
        // unknown queue, or a queue CUPS does not report a device for
        assert_eq!(
            conflict_for("ipp://127.0.0.1:631/printers/other", &queues, true),
            None
        );
        let unknown_device = vec![queue("DeskJet-2600", None)];
        assert_eq!(conflict_for(real, &unknown_device, true), None);
    }
}
