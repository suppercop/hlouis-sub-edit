//! Fluent localization loader and English/Vietnamese message bundles.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use fluent::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use thiserror::Error;
use unic_langid::LanguageIdentifier;

const EN_FTL: &str = include_str!("../locales/en/main.ftl");
const VI_FTL: &str = include_str!("../locales/vi/main.ftl");

/// Supported UI languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Locale {
    #[default]
    En,
    Vi,
}

impl Locale {
    /// BCP-47 language code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Vi => "vi",
        }
    }

    /// Parses a language code (`"en"` or `"vi"`).
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "en" => Some(Self::En),
            "vi" => Some(Self::Vi),
            _ => None,
        }
    }
}

/// Errors produced while initializing Fluent bundles.
#[derive(Debug, Error)]
pub enum I18nError {
    #[error("invalid language identifier `{0}`: {1}")]
    InvalidLangId(String, #[source] unic_langid::LanguageIdentifierError),
    #[error("failed to parse Fluent resource for `{locale}` ({count} syntax error(s))")]
    ParseResource { locale: &'static str, count: usize },
    #[error("failed to register Fluent resource for `{locale}` ({count} conflict(s))")]
    AddResource { locale: &'static str, count: usize },
}

/// Runtime localization catalog supporting instant locale switching.
pub struct I18n {
    locale: Locale,
    en_bundle: FluentBundle<FluentResource>,
    vi_bundle: FluentBundle<FluentResource>,
}

impl I18n {
    /// Builds the localization catalog with embedded English and Vietnamese resources.
    pub fn new(locale: Locale) -> Result<Self, I18nError> {
        let en_bundle = build_bundle("en", EN_FTL)?;
        let vi_bundle = build_bundle("vi", VI_FTL)?;
        Ok(Self {
            locale,
            en_bundle,
            vi_bundle,
        })
    }

    /// Active locale.
    #[must_use]
    pub const fn locale(&self) -> Locale {
        self.locale
    }

    /// Switches active locale at runtime without restarting.
    pub fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
    }

    /// Looks up a plain message by key, falling back to English, then to the key string.
    #[must_use]
    pub fn tr(&self, key: &str) -> String {
        self.tr_with_args(key, None)
    }

    /// Looks up a parameterized message by key.
    #[must_use]
    pub fn tr_args(&self, key: &str, args: &[(&str, FluentValue<'_>)]) -> String {
        let mut fluent_args = FluentArgs::with_capacity(args.len());
        for (name, value) in args {
            fluent_args.set(*name, value.clone());
        }
        self.tr_with_args(key, Some(&fluent_args))
    }

    fn tr_with_args(&self, key: &str, args: Option<&FluentArgs<'_>>) -> String {
        let primary = match self.locale {
            Locale::En => &self.en_bundle,
            Locale::Vi => &self.vi_bundle,
        };
        if let Some(text) = format_from_bundle(primary, key, args) {
            return text;
        }
        if self.locale != Locale::En {
            if let Some(text) = format_from_bundle(&self.en_bundle, key, args) {
                return text;
            }
        }
        key.to_owned()
    }
}

fn build_bundle(
    locale: &'static str,
    source: &'static str,
) -> Result<FluentBundle<FluentResource>, I18nError> {
    let lang_id: LanguageIdentifier = locale
        .parse()
        .map_err(|err| I18nError::InvalidLangId(locale.to_owned(), err))?;
    let resource = FluentResource::try_new(source.to_owned()).map_err(|(_res, errs)| {
        I18nError::ParseResource {
            locale,
            count: errs.len(),
        }
    })?;
    let mut bundle = FluentBundle::new(vec![lang_id]);
    // Disable Unicode bidi isolation marks around interpolated numbers/strings inside UI labels.
    bundle.set_use_isolating(false);
    bundle
        .add_resource(resource)
        .map_err(|errs| I18nError::AddResource {
            locale,
            count: errs.len(),
        })?;
    Ok(bundle)
}

fn format_from_bundle(
    bundle: &FluentBundle<FluentResource>,
    key: &str,
    args: Option<&FluentArgs<'_>>,
) -> Option<String> {
    let msg = bundle.get_message(key)?;
    let pattern = msg.value()?;
    let mut errors = Vec::new();
    let formatted = bundle.format_pattern(pattern, args, &mut errors);
    debug_assert!(
        errors.is_empty(),
        "Fluent formatting error(s) for key `{key}`: {errors:?}"
    );
    Some(formatted.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn extract_keys(ftl: &str) -> BTreeSet<String> {
        ftl.lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    return None;
                }
                trimmed.split_once('=').map(|(k, _)| k.trim().to_owned())
            })
            .collect()
    }

    #[test]
    fn bundles_parse_and_have_key_parity() {
        let i18n = I18n::new(Locale::En).expect("en/vi bundles must parse cleanly");
        let en_keys = extract_keys(EN_FTL);
        let vi_keys = extract_keys(VI_FTL);
        assert_eq!(
            en_keys, vi_keys,
            "en and vi FTL files must contain identical keys"
        );
        assert!(!en_keys.is_empty());

        for key in &en_keys {
            assert!(
                i18n.en_bundle.has_message(key),
                "missing key `{key}` in en bundle"
            );
            assert!(
                i18n.vi_bundle.has_message(key),
                "missing key `{key}` in vi bundle"
            );
        }
    }

    #[test]
    fn runtime_switch_and_args_formatting() {
        let mut i18n = I18n::new(Locale::En).expect("init i18n");
        assert_eq!(i18n.tr("theme-light"), "Light");
        let en_idx = i18n.tr_args("status-indexing", &[("percent", FluentValue::from(42))]);
        assert!(
            en_idx.contains("42%"),
            "unexpected formatted string: {en_idx}"
        );

        i18n.set_locale(Locale::Vi);
        assert_eq!(i18n.tr("theme-light"), "Sáng");
        let vi_idx = i18n.tr_args("status-indexing", &[("percent", FluentValue::from(75))]);
        assert!(
            vi_idx.contains("75%"),
            "unexpected formatted string: {vi_idx}"
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "Fluent formatting error")]
    fn bad_arg_name_triggers_debug_assert() {
        let i18n = I18n::new(Locale::En).expect("init i18n");
        let _ = i18n.tr_args("status-indexing", &[("wrong_arg", FluentValue::from(42))]);
    }
}
