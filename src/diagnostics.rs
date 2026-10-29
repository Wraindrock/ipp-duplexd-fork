//! The "logs and status" text: everything a bug report needs, assembled in one
//! place so the window and the terminal front end show the same thing.
//!
//! It is deliberately plain text: the unit file, what systemctl says, the tail
//! of the journal and one line per virtual printer with its port probe.

use crate::config::Printer;
use crate::discover;
use crate::gui;
use crate::hplip;
use crate::l10n;
use crate::service;
use crate::usb;

/// The whole report, in the language the caller asks for.
pub fn text(printers: &[Printer], locale: &str) -> String {
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
    out.push_str("\n\n--- systemctl --user status ---\n");
    out.push_str(&service::status_text());
    out.push_str("\n\n--- journal ---\n");
    out.push_str(&service::journal_text(80));
    out.push_str("\n\n--- virtual printers ---\n");

    // "is it installed" (a missing dependency) and "is it serving the device"
    // (the hplip conflict) are different questions with different answers.
    let ipp_usb_installed = usb::installed();
    let ipp_usb_serving = usb::serving();
    let known_queues = discover::cups_printers().unwrap_or_default();
    for printer in printers {
        let line = match gui::http_get(&printer.listen, "/", locale) {
            Ok(_) => l10n::tr(
                locale,
                "gui.window.diag_answers",
                &[("listen", printer.listen.clone())],
            ),
            Err(e) => l10n::tr(
                locale,
                "gui.window.unreachable",
                &[("addr", printer.listen.clone()), ("err", e)],
            ),
        };
        out.push_str(&format!("{}: {line}\n", printer.name));
        // A queue hplip drives cannot print while ipp-usb holds the device;
        // the port above answers, so this is the only line that explains it.
        if let Some(conflict) = hplip::conflict_for(&printer.real, &known_queues, ipp_usb_serving) {
            out.push_str(&format!(
                "  {}\n",
                l10n::tr(
                    locale,
                    "gui.window.hplip_hint",
                    &[("queue", conflict.queue)]
                )
            ));
        }
        // A USB printer is only an IPP printer through ipp-usb; say so here,
        // next to the port probe that is already asking why nothing answers.
        if let Some(uri) = usb::missing([printer.real.as_str()], ipp_usb_installed) {
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
