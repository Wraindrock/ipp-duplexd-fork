//! Which language the tools speak.
//!
//! The catalogs themselves live in `locales/*.yml` and are compiled into the
//! binaries by [`rust_i18n`] (see [`crate::i18n!`]); this module only picks a
//! locale and does the runtime interpolation the `t!` macro cannot do when the
//! locale is per-request instead of per-process.

use std::borrow::Cow;

/// Catalog used when nothing else applies, and rust-i18n's fallback.
pub const FALLBACK: &str = "en";

/// Set this environment variable to override the locale of every message.
pub const LANG_ENV: &str = "IPP_DUPLEXD_FORK_LANG";

/// Every locale compiled into the binaries, with its own name for itself.
pub const CATALOG: &[(&str, &str)] = &[("en", "English"), ("zh-CN", "简体中文")];

/// The codes of all compiled-in locales, in catalog order.
pub fn codes() -> Vec<&'static str> {
    CATALOG.iter().map(|(code, _)| *code).collect()
}

/// "en (English), zh-CN (简体中文)" — for the error that lists what is known.
pub fn describe_all() -> String {
    CATALOG
        .iter()
        .map(|(code, name)| format!("{code} ({name})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// "English", "简体中文", ... — the name of `code` in that language.
pub fn native_name(code: &str) -> Option<&'static str> {
    CATALOG
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(_, name)| *name)
}

/// Turn a locale tag into a catalog code.
///
/// Accepts the shapes the environment and HTTP throw at us: `zh_CN.UTF-8`,
/// `zh-CN`, `en_US@modifier`, `C`, `POSIX`. A region we have no catalog for
/// falls back to its primary language, so `zh-TW` and `zh-Hant` also get the
/// (Simplified) Chinese catalog.
pub fn normalize(tag: &str) -> Option<&'static str> {
    let tag = tag.split(['.', '@']).next().unwrap_or("").trim();
    let tag = tag.replace('_', "-");
    let tag = tag.trim_matches('-');
    if tag.is_empty() {
        return None;
    }
    if tag.eq_ignore_ascii_case("C") || tag.eq_ignore_ascii_case("POSIX") {
        return Some(FALLBACK);
    }

    let lowered = tag.to_ascii_lowercase();
    if let Some((code, _)) = CATALOG
        .iter()
        .find(|(code, _)| code.to_ascii_lowercase() == lowered)
    {
        return Some(code);
    }

    let primary = lowered.split('-').next()?;
    CATALOG
        .iter()
        .find(|(code, _)| {
            code.to_ascii_lowercase()
                .split('-')
                .next()
                .is_some_and(|known| known == primary)
        })
        .map(|(code, _)| *code)
}

/// The locale the process should use, taken from the environment.
pub fn from_env() -> Option<&'static str> {
    from_lookup(|name| std::env::var(name).ok())
}

/// First of `IPP_DUPLEXD_FORK_LANG`, `$LANGUAGE`, `$LC_ALL`, `$LC_MESSAGES` or
/// `$LANG` that names a locale we have a catalog for.
///
/// `$LANGUAGE` is a colon-separated priority list, so it is walked in order.
pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    for name in [LANG_ENV, "LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        let Some(value) = lookup(name) else {
            continue;
        };
        for tag in value.split(':') {
            if let Some(code) = normalize(tag) {
                return Some(code);
            }
        }
    }
    None
}

/// Make `code` the language of every subsequent message and return the code
/// actually in effect (the fallback for anything unknown).
pub fn apply(code: &str) -> &'static str {
    let code = normalize(code).unwrap_or(FALLBACK);
    rust_i18n::set_locale(code);
    code
}

/// Pick a catalog code for an HTTP `Accept-Language` value.
///
/// Honours q-values and the order of equally weighted entries; `*` means
/// "anything" and yields the fallback; unknown languages are skipped.
pub fn negotiate(accept_language: &str) -> &'static str {
    let mut weighted: Vec<(u32, usize, &str)> = Vec::new();
    for (position, entry) in accept_language.split(',').enumerate() {
        let mut fields = entry.split(';');
        let tag = fields.next().unwrap_or("").trim();
        if tag.is_empty() {
            continue;
        }
        let mut quality = 1000;
        for field in fields {
            let field = field.trim();
            if let Some(value) = field.strip_prefix("q=") {
                quality = parse_quality(value);
            }
        }
        if quality > 0 {
            weighted.push((quality, position, tag));
        }
    }
    // Highest quality first, ties broken by the order in the header.
    weighted.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));

    for (_, _, tag) in &weighted {
        if *tag == "*" {
            return FALLBACK;
        }
        if let Some(code) = normalize(tag) {
            return code;
        }
    }
    FALLBACK
}

/// `0`, `0.8`, `1` -> 0, 800, 1000 (no float parsing, no surprises).
fn parse_quality(value: &str) -> u32 {
    let value = value.trim();
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let whole: u32 = whole.parse().unwrap_or(1);
    let mut fraction = fraction.to_owned();
    fraction.truncate(3);
    while fraction.len() < 3 {
        fraction.push('0');
    }
    let fraction: u32 = fraction.parse().unwrap_or(0);
    whole
        .saturating_mul(1000)
        .saturating_add(fraction)
        .min(1000)
}

/// `key` in `locale`, with every `%{name}` placeholder replaced from `vars`.
///
/// Falls back to English through rust-i18n's fallback list, then to the key
/// itself (which makes a typo visible instead of blank).
pub fn tr(locale: &str, key: &str, vars: &[(&'static str, String)]) -> String {
    let template: Cow<'static, str> =
        crate::_rust_i18n_try_translate(locale, key).unwrap_or_else(|| Cow::Owned(key.to_owned()));
    if vars.is_empty() {
        return template.into_owned();
    }
    let names: Vec<&str> = vars.iter().map(|(name, _)| *name).collect();
    let values: Vec<String> = vars.iter().map(|(_, value)| value.clone()).collect();
    rust_i18n::replace_patterns(&template, &names, &values)
}

/// `key` in `locale`, for keys without placeholders.
pub fn text(locale: &str, key: &str) -> String {
    tr(locale, key, &[])
}

/// The distinct `%{name}` placeholders in `text`, sorted (used by the catalog
/// completeness test).
pub fn placeholders(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("%{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            break;
        };
        let name = &after[..end];
        if !name.is_empty() && !found.iter().any(|seen| seen == name) {
            found.push(name.to_owned());
        }
        rest = &after[end + 1..];
    }
    found.sort();
    found
}

/// Every key the compiled catalog has for `locale`, sorted.
///
/// The locale files are compiled into the binary, so this is also the list of
/// what a translator still owes.
pub fn keys(locale: &str) -> Vec<String> {
    let mut keys: Vec<String> = crate::_rust_i18n_backend()
        .messages_for_locale(locale)
        .unwrap_or_default()
        .into_iter()
        .map(|(key, _)| key.into_owned())
        .collect();
    keys.sort();
    keys
}

/// Whether the catalog has `key` for `locale` (no fallback involved).
pub fn has_key(locale: &str, key: &str) -> bool {
    crate::_rust_i18n_backend().translate(locale, key).is_some()
}

/// The locales whose files were compiled into the binary.
pub fn compiled_locales() -> Vec<String> {
    let mut codes: Vec<String> = crate::_rust_i18n_available_locales()
        .iter()
        .map(|code| code.to_string())
        .collect();
    codes.sort();
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_accepts_environment_shapes() {
        assert_eq!(normalize("en"), Some("en"));
        assert_eq!(normalize("en_US.UTF-8"), Some("en"));
        assert_eq!(normalize("zh_CN.UTF-8"), Some("zh-CN"));
        assert_eq!(normalize("zh-CN"), Some("zh-CN"));
        assert_eq!(normalize("zh-tw"), Some("zh-CN"));
        assert_eq!(normalize("C"), Some("en"));
        assert_eq!(normalize("POSIX"), Some("en"));
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("de_DE.UTF-8"), None);
    }

    #[test]
    fn from_lookup_prefers_the_more_specific_variable() {
        let lookup = |name: &str| match name {
            "LANG" => Some("de_DE.UTF-8".to_owned()),
            "LC_ALL" => Some("zh_CN.UTF-8".to_owned()),
            LANG_ENV => Some("en".to_owned()),
            _ => None,
        };
        assert_eq!(from_lookup(lookup), Some("en"));

        let no_override = |name: &str| match name {
            "LANG" => Some("zh_CN.UTF-8".to_owned()),
            "LC_ALL" => Some("de_DE.UTF-8".to_owned()),
            _ => None,
        };
        assert_eq!(from_lookup(no_override), Some("zh-CN"));

        let language_list = |name: &str| match name {
            "LANGUAGE" => Some("de:zh_CN:en".to_owned()),
            _ => None,
        };
        assert_eq!(from_lookup(language_list), Some("zh-CN"));

        assert_eq!(from_lookup(|_| None), None);
    }

    #[test]
    fn negotiate_honours_quality_values() {
        assert_eq!(negotiate("zh-CN,zh;q=0.9,en;q=0.8"), "zh-CN");
        assert_eq!(negotiate("en;q=0.5,zh-CN;q=0.9"), "zh-CN");
        assert_eq!(negotiate("de-DE,zh-CN;q=0.7"), "zh-CN");
        assert_eq!(negotiate("de-DE"), FALLBACK);
        assert_eq!(negotiate("*"), FALLBACK);
        assert_eq!(negotiate(""), FALLBACK);
        // q=0 means "not acceptable".
        assert_eq!(negotiate("zh-CN;q=0,en;q=0.1"), "en");
    }

    #[test]
    fn placeholders_are_deduplicated_and_sorted() {
        assert_eq!(
            placeholders("%{name} has %{count} of %{name}"),
            vec!["count".to_owned(), "name".to_owned()]
        );
        assert!(placeholders("no placeholders").is_empty());
        assert!(placeholders("%{unterminated").is_empty());
    }

    #[test]
    fn tr_translates_and_interpolates() {
        assert_eq!(text("en", "daemon.state.pending"), "pending");
        assert_eq!(text("zh-CN", "daemon.state.pending"), "等待中");
        // Unknown locales fall back to English rather than to nothing.
        assert_eq!(text("de-DE", "daemon.state.pending"), "pending");

        let vars = [("what", "--poll".to_owned())];
        assert_eq!(
            tr("en", "daemon.err.needs_value", &vars),
            "--poll needs a value"
        );
        assert_eq!(
            tr("zh-CN", "daemon.err.needs_value", &vars),
            "--poll 缺少取值"
        );
    }

    #[test]
    fn every_catalog_locale_is_compiled_in() {
        let compiled = crate::_rust_i18n_available_locales();
        let mut expected = codes();
        expected.sort_unstable();
        let mut compiled: Vec<String> = compiled.iter().map(|code| code.to_string()).collect();
        compiled.sort();
        assert_eq!(compiled, expected);
        for code in codes() {
            assert!(native_name(code).is_some(), "{code} has no display name");
        }
    }
}
