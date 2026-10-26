//! ipp-duplexd-fork library surface: the IPP codec/client and the language
//! handling, shared with the binaries and the integration tests.
pub mod config;
pub mod gui;
pub mod ipp;
pub mod l10n;
pub mod service;

rust_i18n::i18n!("locales", fallback = "en");
