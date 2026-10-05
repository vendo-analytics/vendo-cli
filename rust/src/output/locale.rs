//! Locale-aware formatting as Node does it (VE-3728): `toLocaleDateString()`,
//! `toLocaleTimeString()` and `Number#toLocaleString()` in the default locale, which Node's ICU
//! takes from `LC_ALL`, `LC_MESSAGES` or `LANG`. ICU4X does the formatting; the `yMd` date
//! pattern comes from [`super::ymd_patterns`] (see there for why).

use fixed_decimal::{Decimal, FloatPrecision, SignedRoundingMode, UnsignedRoundingMode};
use icu_calendar::{AnyCalendarKind, Date, Gregorian};
use icu_datetime::{
    DateTimeFormatter, NoCalendarFormatter,
    fieldsets::{T, YMD},
    options::{TimePrecision, YearStyle},
    pattern::{DateTimePattern, FixedCalendarDateTimeNames},
};
use icu_decimal::DecimalFormatter;
use icu_locale_core::{DataLocale, Locale, locale};
use icu_locale_fallback::LocaleFallbacker;
use icu_time::Time;
use jiff::tz::TimeZone;
use writeable::TryWriteable;

use super::ymd_patterns::{LOCALES, PATTERNS};

/// The locale Node's ICU defaults to: `LC_ALL`, else `LC_MESSAGES`, else `LANG`, where a
/// variable set to "" still counts (and gives `und`). `C` and `POSIX` (with any charset) are
/// `en-US`, as is no variable at all. The charset (`.UTF-8`) and modifier (`@euro`) are dropped.
pub fn default_locale(var: impl Fn(&str) -> Option<String>) -> Locale {
    let Some(id) = var("LC_ALL").or_else(|| var("LC_MESSAGES")).or_else(|| var("LANG")) else {
        return locale!("en-US");
    };
    let base = id.split(['.', '@']).next().unwrap_or_default();
    if base == "C" || base == "POSIX" {
        return locale!("en-US");
    }
    base.replace('_', "-").parse().unwrap_or(Locale::UNKNOWN)
}

thread_local! {
    /// The formatters for the process's locale, read once from the environment (the CLI runs on
    /// one thread). Under `cargo test` it is always `en-US`, so tests don't depend on the
    /// developer's `LANG`.
    static CURRENT: LocaleFormat = {
        let var = |key: &str| std::env::var_os(key).map(|v| v.to_string_lossy().into_owned());
        let locale = if cfg!(test) { default_locale(|_| None) } else { default_locale(var) };
        LocaleFormat::new(&locale)
    };
}

/// Run `f` with the process locale's formatters.
pub fn with_current<R>(f: impl FnOnce(&LocaleFormat) -> R) -> R {
    CURRENT.with(f)
}

/// ICU4X formatters for one locale.
pub struct LocaleFormat {
    /// Calendar-aware year/month/day, for locales without a Gregorian `yMd` pattern.
    date: DateTimeFormatter<YMD>,
    /// Node's `yMd` pattern, for locales on the Gregorian calendar.
    ymd: Option<(FixedCalendarDateTimeNames<Gregorian, YMD>, DateTimePattern)>,
    time: NoCalendarFormatter<T>,
    number: DecimalFormatter,
}

impl LocaleFormat {
    pub fn new(locale: &Locale) -> Self {
        // Compiled data falls back to root for any locale, so `und` always works.
        let date = DateTimeFormatter::try_new(locale.into(), YMD::short().with_year_style(YearStyle::Full))
            .or_else(|_| DateTimeFormatter::try_new(Locale::UNKNOWN.into(), YMD::short()))
            .expect("compiled data has root date patterns");
        let time_fields = T::medium().with_time_precision(TimePrecision::Second);
        let time = NoCalendarFormatter::try_new(locale.into(), time_fields)
            .or_else(|_| NoCalendarFormatter::try_new(Locale::UNKNOWN.into(), time_fields))
            .expect("compiled data has root time patterns");
        let number = DecimalFormatter::try_new(locale.into(), Default::default())
            .or_else(|_| DecimalFormatter::try_new(Locale::UNKNOWN.into(), Default::default()))
            .expect("compiled data has root decimal symbols");
        let gregorian = matches!(AnyCalendarKind::try_new(locale.into()), Ok(AnyCalendarKind::Gregorian));
        let ymd = gregorian
            .then(|| {
                let mut names = FixedCalendarDateTimeNames::try_new(locale.into()).ok()?;
                let pattern: DateTimePattern = ymd_pattern(locale).parse().ok()?;
                // Loads what the pattern needs (numeric month formatting) once.
                names.include_for_pattern(&pattern).ok()?;
                Some((names, pattern))
            })
            .flatten();
        LocaleFormat { date, ymd, time, number }
    }

    /// `new Date(ms).toLocaleDateString()` in `tz`. Outside the years ICU4X supports
    /// (-9999 to 9999) it gives the ISO date instead.
    pub fn date(&self, ms: i64, tz: &TimeZone) -> String {
        let Some(civil) = local(ms, tz) else { return iso_date(ms) };
        let Ok(date) = Date::try_new_iso(i32::from(civil.year()), civil.month() as u8, civil.day() as u8) else {
            return iso_date(ms);
        };
        let text = match &self.ymd {
            Some((names, pattern)) => names
                .with_pattern_unchecked(pattern)
                .format(&date.to_calendar(Gregorian))
                .try_write_to_string()
                .unwrap_or_else(|(_, lossy)| lossy)
                .into_owned(),
            None => self.date.format(&date).to_string(),
        };
        v8_spaces(text)
    }

    /// `new Date(ms).toLocaleTimeString()` in `tz`.
    pub fn time(&self, ms: i64, tz: &TimeZone) -> String {
        let Some(civil) = local(ms, tz) else { return "Invalid Date".to_string() };
        let Ok(time) = Time::try_new(civil.hour() as u8, civil.minute() as u8, civil.second() as u8, 0) else {
            return "Invalid Date".to_string();
        };
        v8_spaces(self.time.format(&time).to_string())
    }

    /// `n.toLocaleString()`: at most three fraction digits, rounding half away from zero from
    /// the shortest decimal that round-trips, as ICU does.
    pub fn number(&self, n: f64) -> String {
        if n.is_nan() {
            return "NaN".to_string();
        }
        if n.is_infinite() {
            return if n > 0.0 { "∞" } else { "-∞" }.to_string();
        }
        let Ok(mut decimal) = Decimal::try_from_f64(n, FloatPrecision::RoundTrip) else {
            return crate::output::js_number_string(n);
        };
        decimal.round_with_mode(-3, SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand));
        decimal.absolute.trim_end();
        self.number.format(&decimal).to_string()
    }
}

/// The local wall-clock time of `ms`, within jiff's range.
fn local(ms: i64, tz: &TimeZone) -> Option<jiff::civil::DateTime> {
    Some(tz.to_datetime(jiff::Timestamp::from_millisecond(ms).ok()?))
}

fn iso_date(ms: i64) -> String {
    let iso = crate::js_date::iso_string(ms);
    iso.split('T').next().unwrap_or(&iso).to_string()
}

/// V8 reverts ICU's U+202F NARROW NO-BREAK SPACE to a plain space in date and time output
/// (crbug.com/1414292), e.g. before "PM".
fn v8_spaces(text: String) -> String {
    if text.contains('\u{202f}') { text.replace('\u{202f}', " ") } else { text }
}

/// Node's `yMd` pattern for `locale`: the locale itself, then its ICU4X fallback chain, then root.
fn ymd_pattern(locale: &Locale) -> &'static str {
    let lookup =
        |tag: &str| LOCALES.binary_search_by_key(&tag, |(t, _)| t).ok().map(|i| PATTERNS[LOCALES[i].1 as usize]);
    if let Some(pattern) = lookup(&locale.to_string()) {
        return pattern;
    }
    let fallbacker = LocaleFallbacker::new();
    let mut chain = fallbacker.for_config(Default::default()).fallback_for(DataLocale::from(locale));
    loop {
        let step = chain.get();
        if let Some(pattern) = lookup(&step.to_string()) {
            return pattern;
        }
        if step.is_unknown() {
            return "y-MM-dd";
        }
        chain.step();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |key| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    #[test]
    fn the_default_locale_comes_from_the_environment_like_nodes_icu() {
        for (vars, expected) in [
            (&[][..], "en-US"),
            (&[("LANG", "en_AU.UTF-8")], "en-AU"),
            (&[("LANG", "de_DE")], "de-DE"),
            (&[("LANG", "de")], "de"),
            (&[("LANG", "de-DE")], "de-DE"),
            (&[("LANG", "EN_us.utf8")], "en-US"),
            (&[("LANG", "en_US.ISO8859-1")], "en-US"),
            (&[("LANG", "C")], "en-US"),
            (&[("LANG", "C.UTF-8")], "en-US"),
            (&[("LANG", "POSIX")], "en-US"),
            (&[("LANG", "de_DE@euro")], "de-DE"),
            (&[("LANG", "xx_YY.UTF-8")], "xx-YY"),
            // Set but empty is not unset: ICU reads the value as is.
            (&[("LANG", "")], "und"),
            (&[("LC_ALL", ""), ("LANG", "de_DE.UTF-8")], "und"),
            (&[("LC_MESSAGES", ""), ("LANG", "de_DE.UTF-8")], "und"),
            (&[("LC_ALL", "de_DE.UTF-8"), ("LANG", "en_AU.UTF-8")], "de-DE"),
            (&[("LC_MESSAGES", "ja_JP.UTF-8"), ("LANG", "en_AU.UTF-8")], "ja-JP"),
            (&[("LC_ALL", "C"), ("LANG", "de_DE.UTF-8")], "en-US"),
            (&[("LC_MESSAGES", "C"), ("LANG", "de_DE.UTF-8")], "en-US"),
            // Node ignores the other categories and LANGUAGE.
            (
                &[
                    ("LC_TIME", "de_DE.UTF-8"),
                    ("LC_NUMERIC", "de_DE.UTF-8"),
                    ("LANGUAGE", "de"),
                    ("LANG", "en_AU.UTF-8"),
                ],
                "en-AU",
            ),
            (&[("LC_CTYPE", "de_DE.UTF-8")], "en-US"),
        ] {
            assert_eq!(default_locale(env(vars)).to_string(), expected, "{vars:?}");
        }
    }

    /// Every value in the fixture, generated from Node by `scripts/gen-locale-fixture.mjs`.
    #[test]
    fn dates_times_and_numbers_match_node_for_each_lang_and_time_zone() {
        let fixture: Value = serde_json::from_str(include_str!("../../tests/fixtures/node-locale.json")).unwrap();
        let mut mismatches = Vec::new();
        let mut checked = 0;
        for run in fixture["runs"].as_array().unwrap() {
            let (lang, tz) = (run["lang"].as_str().unwrap(), run["tz"].as_str().unwrap());
            let locale = default_locale(env(&[("LANG", lang)]));
            assert_eq!(locale.to_string(), run["locale"].as_str().unwrap(), "{lang}");
            let format = LocaleFormat::new(&locale);
            let zone = jiff::tz::TimeZone::get(tz).unwrap();
            let mut check = |what: &str, got: String, want: &Value| {
                checked += 1;
                if got != want.as_str().unwrap() {
                    mismatches.push(format!("{lang} {tz} {what}: got {got:?}, Node {want}"));
                }
            };
            for row in run["dates"].as_array().unwrap() {
                let ms = row[0].as_i64().unwrap();
                check(&format!("date {ms}"), format.date(ms, &zone), &row[1]);
                check(&format!("time {ms}"), format.time(ms, &zone), &row[2]);
            }
            for row in run["numbers"].as_array().unwrap() {
                let source = row[0].as_str().unwrap();
                let n: f64 = if source == "-0" { -0.0 } else { source.parse().unwrap() };
                check(&format!("number {source}"), format.number(n), &row[1]);
            }
        }
        assert!(mismatches.is_empty(), "{} of {checked} differ:\n{}", mismatches.len(), mismatches.join("\n"));
        assert_eq!(checked, 18 * (21 * 2 + 38));
    }

    #[test]
    fn non_finite_numbers_print_like_javascript() {
        let format = LocaleFormat::new(&"en-US".parse().unwrap());
        assert_eq!(format.number(f64::NAN), "NaN");
        assert_eq!(format.number(f64::INFINITY), "∞");
        assert_eq!(format.number(f64::NEG_INFINITY), "-∞");
    }

    #[test]
    fn dates_outside_icu4xs_range_fall_back_to_iso() {
        let format = LocaleFormat::new(&"en-US".parse().unwrap());
        assert_eq!(format.date(8_640_000_000_000_000, &jiff::tz::TimeZone::UTC), "+275760-09-13");
    }
}
