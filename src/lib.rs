//! ipp-duplexd-fork library surface: the IPP codec/client and the language
//! handling, shared with the binaries and the integration tests.
pub mod config;
pub mod discover;
pub mod gui;
pub mod hplip;
pub mod ipp;
pub mod l10n;
pub mod queues;
pub mod service;
pub mod usb;

rust_i18n::i18n!("locales", fallback = "en");
