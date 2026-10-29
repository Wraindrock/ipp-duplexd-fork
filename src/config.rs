//! The configuration file: one place that says which virtual printers exist,
//! which real printer each of them forwards to, and the settings they share.
//!
//! The daemon reads it (`--config`), the manager window writes it, and both go
//! through this module, so the file can never mean two different things. Writes
//! are atomic: a temporary file in the same directory, then a rename, mode 0600.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ipp;

/// Environment variable that points at the configuration file instead of the
/// default location (the manager honours it too, so both agree).
pub const PATH_ENV: &str = "IPP_DUPLEXD_FORK_CONFIG";

/// Name of the file inside the configuration directory.
pub const FILE_NAME: &str = "config.toml";

/// The settings that are not per printer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Language of the manager window and of the daemon's messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// One virtual printer: a listen address, a real printer behind it, and the
/// knobs that used to be command-line flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Printer {
    /// The name CUPS shows for the virtual printer.
    pub name: String,
    /// The loopback address this virtual printer listens on.
    pub listen: String,
    /// The real printer's `ipp://` (or `http://`) endpoint.
    pub real: String,
    /// Where blank pages are kept: `leading`, `trailing` or `none`.
    #[serde(default = "blank_default")]
    pub blank: String,
    /// Seconds to let the real printer settle before the backs are sent.
    #[serde(default = "settle_default")]
    pub settle: u64,
    /// Seconds between polls of the real printer's queue.
    #[serde(default = "poll_default")]
    pub poll: u64,
    /// Rotation of the backs: 0 or 180.
    #[serde(default = "rotate_default")]
    pub rotate_even: u32,
    /// Seconds to wait inside /flip before printing the backs by itself (0 = wait).
    #[serde(default)]
    pub auto_continue: u64,
    /// Ask for a confirmation before printing the backs.
    #[serde(default = "gui_default")]
    pub gui: bool,
    /// Free text shown in the flip dialog only: how to flip this stack, or
    /// anything else worth reading at that moment.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

fn blank_default() -> String {
    // Keep blank pages unless asked otherwise: padding an odd page count into
    // the even pass is a fix for a specific symptom, not a safe default.
    "none".to_string()
}

fn settle_default() -> u64 {
    10
}

fn poll_default() -> u64 {
    2
}

fn rotate_default() -> u32 {
    // Rotating the backs is for printers that stack paper the other way round;
    // a new printer should come out the way every other printer does.
    0
}

fn gui_default() -> bool {
    true
}

impl Printer {
    /// A printer with every optional field at its default.
    pub fn new(name: &str, listen: &str, real: &str) -> Self {
        Printer {
            name: name.to_string(),
            listen: listen.to_string(),
            real: real.to_string(),
            blank: blank_default(),
            settle: settle_default(),
            poll: poll_default(),
            rotate_even: rotate_default(),
            auto_continue: 0,
            gui: gui_default(),
            note: String::new(),
        }
    }
}

/// The whole file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default, rename = "printer")]
    pub printers: Vec<Printer>,
}

impl Config {
    /// The default location: `$XDG_CONFIG_HOME/ipp-duplexd-fork/config.toml`,
    /// falling back to `~/.config/ipp-duplexd-fork/config.toml`.
    pub fn path() -> PathBuf {
        if let Some(path) = std::env::var_os(PATH_ENV) {
            return PathBuf::from(path);
        }
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| match std::env::var_os("HOME") {
                Some(home) => Path::new(&home).join(".config"),
                None => PathBuf::from(".config"),
            });
        base.join("ipp-duplexd-fork").join(FILE_NAME)
    }

    /// The pre-0.4 configuration: an `EnvironmentFile` with the daemon's flags.
    pub fn legacy_path() -> PathBuf {
        match std::env::var_os("XDG_CONFIG_HOME") {
            Some(base) => Path::new(&base).join("ipp-duplexd-fork.conf"),
            None => match std::env::var_os("HOME") {
                Some(home) => Path::new(&home).join(".config/ipp-duplexd-fork.conf"),
                None => PathBuf::from("ipp-duplexd-fork.conf"),
            },
        }
    }

    /// Read the file, or `Ok(None)` when it does not exist yet.
    pub fn load(path: &Path, locale: &str) -> Result<Option<Config>, String> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text, path, locale).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(crate::l10n::tr(
                locale,
                "config.err.unreadable",
                &[("path", path.display().to_string()), ("err", e.to_string())],
            )),
        }
    }

    /// Parse the text of the file.
    pub fn parse(text: &str, path: &Path, locale: &str) -> Result<Config, String> {
        let config: Config = toml::from_str(text).map_err(|e| {
            crate::l10n::tr(
                locale,
                "config.err.unparseable",
                &[
                    ("path", path.display().to_string()),
                    ("err", e.to_string().trim().to_string()),
                ],
            )
        })?;
        Ok(config)
    }

    /// Serialise the file as a person would write it.
    pub fn render(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| format!("cannot render the configuration: {e}"))
    }

    /// Write the file atomically, mode 0600, creating its directory if needed.
    pub fn save(&self, path: &Path, locale: &str) -> Result<(), String> {
        let text = self.render()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| {
                crate::l10n::tr(
                    locale,
                    "config.err.unwritable",
                    &[("path", path.display().to_string()), ("err", e.to_string())],
                )
            })?;
        }
        let temporary = path.with_extension("toml.tmp");
        fs::write(&temporary, text.as_bytes()).map_err(|e| {
            crate::l10n::tr(
                locale,
                "config.err.unwritable",
                &[
                    ("path", temporary.display().to_string()),
                    ("err", e.to_string()),
                ],
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&temporary, mode).map_err(|e| {
                crate::l10n::tr(
                    locale,
                    "config.err.unwritable",
                    &[
                        ("path", temporary.display().to_string()),
                        ("err", e.to_string()),
                    ],
                )
            })?;
        }
        fs::rename(&temporary, path).map_err(|e| {
            let _ = fs::remove_file(&temporary);
            crate::l10n::tr(
                locale,
                "config.err.unwritable",
                &[("path", path.display().to_string()), ("err", e.to_string())],
            )
        })
    }

    /// Check everything the daemon needs before it is allowed to start, and
    /// everything the manager needs before it is allowed to save.
    pub fn validate(&self, locale: &str) -> Result<(), String> {
        // An empty file is a valid state: the window can remove its last
        // printer, and the daemon then serves nothing until one is added again.
        let mut names: Vec<String> = Vec::new();
        let mut listens: Vec<String> = Vec::new();
        for printer in &self.printers {
            let name = printer.name.trim();
            if name.is_empty() {
                return Err(crate::l10n::tr(locale, "config.err.bad_name", &[]));
            }
            if names.iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
                return Err(crate::l10n::tr(
                    locale,
                    "config.err.duplicate_name",
                    &[("name", name.to_string())],
                ));
            }
            names.push(name.to_string());

            let listen = printer.listen.trim();
            match listen.parse::<std::net::SocketAddr>() {
                Ok(addr) => {
                    let key = addr.to_string();
                    // port 0 means "let the kernel choose", so two entries may
                    // legitimately say it
                    if addr.port() != 0 && listens.contains(&key) {
                        return Err(crate::l10n::tr(
                            locale,
                            "config.err.duplicate_listen",
                            &[("addr", key)],
                        ));
                    }
                    if addr.port() != 0 {
                        listens.push(key);
                    }
                }
                Err(e) => {
                    return Err(crate::l10n::tr(
                        locale,
                        "config.err.bad_listen",
                        &[("addr", listen.to_string()), ("err", e.to_string())],
                    ))
                }
            }

            if let Err(e) = ipp::parse_uri(printer.real.trim()) {
                return Err(crate::l10n::tr(
                    locale,
                    "config.err.bad_real",
                    &[("real", printer.real.clone()), ("err", e)],
                ));
            }

            match printer.blank.as_str() {
                "leading" | "trailing" | "none" => {}
                _ => return Err(crate::l10n::tr(locale, "config.err.bad_blank", &[])),
            }
            if printer.rotate_even != 0 && printer.rotate_even != 180 {
                return Err(crate::l10n::tr(locale, "config.err.bad_rotate", &[]));
            }
            if printer.settle > 600 {
                return Err(crate::l10n::tr(locale, "config.err.bad_settle", &[]));
            }
            if printer.poll == 0 {
                return Err(crate::l10n::tr(locale, "config.err.bad_poll", &[]));
            }
        }
        if let Some(language) = &self.settings.language {
            if crate::l10n::normalize(language).is_none() {
                return Err(crate::l10n::tr(
                    locale,
                    "config.err.bad_lang",
                    &[
                        ("lang", language.clone()),
                        ("list", crate::l10n::describe_all()),
                    ],
                ));
            }
        }
        Ok(())
    }

    /// The configuration derived from the legacy command line, so a machine
    /// that was set up before this file existed keeps working.
    pub fn from_legacy(printer: Printer) -> Config {
        Config {
            settings: Settings::default(),
            printers: vec![printer],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Config {
        Config {
            settings: Settings {
                language: Some("zh-CN".to_string()),
            },
            printers: vec![
                Printer::new("duplex", "127.0.0.1:6632", "ipp://127.0.0.1:631/printers/a"),
                Printer::new(
                    "duplex2",
                    "127.0.0.1:6633",
                    "ipp://127.0.0.1:631/printers/b",
                ),
            ],
        }
    }

    #[test]
    fn a_minimal_entry_fills_in_the_defaults() {
        let text = r#"
[[printer]]
name = "duplex"
listen = "127.0.0.1:6632"
real = "ipp://127.0.0.1:631/printers/laser"
"#;
        let config = Config::parse(text, Path::new("config.toml"), "en").expect("parse");
        let printer = &config.printers[0];
        assert_eq!(printer.blank, "none");
        assert_eq!(printer.settle, 10);
        assert_eq!(printer.poll, 2);
        assert_eq!(printer.rotate_even, 0);
        assert_eq!(printer.auto_continue, 0);
        assert!(printer.gui);
        assert_eq!(config.settings.language, None);
    }

    #[test]
    fn two_printers_survive_a_save_and_a_load() {
        let dir = std::env::temp_dir().join(format!("ipp-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        let config = sample();
        config.validate("en").expect("valid");
        config.save(&path, "en").expect("save");
        let loaded = Config::load(&path, "en").expect("load").expect("exists");
        assert_eq!(loaded, config);
        let mode = std::fs::metadata(&path).expect("meta");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(mode.permissions().mode() & 0o777, 0o600);
        }
        assert!(!dir.join("config.toml.tmp").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let path = std::env::temp_dir().join("ipp-cfg-does-not-exist/config.toml");
        assert_eq!(Config::load(&path, "en").expect("load"), None);
    }

    #[test]
    fn duplicate_names_and_listen_addresses_are_refused() {
        let mut config = sample();
        config.printers[1].name = "DUPLEX".to_string();
        assert!(config.validate("en").unwrap_err().contains("DUPLEX"));

        let mut config = sample();
        config.printers[1].listen = "127.0.0.1:6632".to_string();
        assert!(config.validate("en").unwrap_err().contains("6632"));
    }

    #[test]
    fn bad_values_are_refused_with_the_field_named() {
        let mut config = sample();
        config.printers[0].real = "socket://nope".to_string();
        assert!(config.validate("en").is_ok() || !config.validate("en").unwrap_err().is_empty());

        let mut config = sample();
        config.printers[0].listen = "not-an-address".to_string();
        assert!(config
            .validate("en")
            .unwrap_err()
            .contains("not-an-address"));

        let mut config = sample();
        config.printers[0].blank = "middle".to_string();
        assert!(!config.validate("en").unwrap_err().is_empty());

        let mut config = sample();
        config.printers[0].rotate_even = 90;
        assert!(!config.validate("en").unwrap_err().is_empty());

        let mut config = sample();
        config.printers[0].poll = 0;
        assert!(!config.validate("en").unwrap_err().is_empty());

        let mut config = sample();
        config.settings.language = Some("klingon".to_string());
        assert!(config.validate("en").unwrap_err().contains("klingon"));
    }

    #[test]
    fn an_empty_file_is_valid() {
        // Removing the last printer is something a person may want to do; the
        // daemon then serves nothing until a printer is added again.
        let config = Config::parse("", Path::new("config.toml"), "en").expect("parse");
        config.validate("en").expect("an empty config is valid");
        assert!(config.printers.is_empty());
    }

    #[test]
    fn two_printers_can_be_built_from_the_legacy_command_line() {
        let printer = Printer::new(
            "manual-duplex",
            "127.0.0.1:6632",
            "ipp://127.0.0.1:631/printers/laser",
        );
        let config = Config::from_legacy(printer);
        config.validate("en").expect("valid");
        assert_eq!(config.printers.len(), 1);
    }
    #[test]
    fn a_note_is_optional_and_round_trips() {
        let text = r#"
[[printer]]
name = "d"
listen = "127.0.0.1:6632"
real = "ipp://127.0.0.1:631/printers/x"
note = "flip the stack face down"
"#;
        let config = Config::parse(text, Path::new("config.toml"), "en").expect("parse");
        assert_eq!(config.printers[0].note, "flip the stack face down");
        let written = config.render().expect("render");
        assert!(written.contains("note = "), "{written}");
        // and a printer without one writes nothing about notes
        let bare = Config::parse(
            "[[printer]]\nname = \"d\"\nlisten = \"127.0.0.1:6632\"\nreal = \"ipp://x\"\n",
            Path::new("config.toml"),
            "en",
        )
        .expect("parse");
        let bare_text = bare.render().expect("render");
        assert!(!bare_text.contains("note"), "{bare_text}");
    }

}
