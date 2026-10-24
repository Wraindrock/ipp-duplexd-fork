//! ipp-duplexd-fork library surface: the IPP codec/client and the language
//! handling, shared with the binaries and the integration tests.
pub mod ipp;
pub mod l10n;

rust_i18n::i18n!("locales", fallback = "en");
