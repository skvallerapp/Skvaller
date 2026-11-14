//! Dates and times as the user reads them: in the system's time zone, formatted for the
//! system's region.
//!
//! The settings can pick the clock (24 or 12 hours) and the order of day and month.
//! Month and weekday names follow the region either way.

use std::fmt;
use std::ops::Range;

use icu_datetime::fieldsets;
use icu_datetime::input::{Date, DateTime, Time};
use icu_datetime::preferences::{CalendarAlgorithm, HourCycle};
use icu_datetime::{DateTimeFormatter, parts};
use icu_locale_core::Locale;
use jiff::Timestamp;
use jiff::civil;
use jiff::tz::TimeZone;
use skvaller_app::{Clock, DateFormat};
use writeable::{Part, PartsWrite, Writeable};

use crate::i18n::t;

/// Formats dates for one locale, time zone and pair of settings.
#[derive(Debug)]
pub struct Dates {
    zone: TimeZone,
    order: DateFormat,
    /// Hour and minute.
    time: DateTimeFormatter<fieldsets::T>,
    /// "9 Nov": day and month.
    day: DateTimeFormatter<fieldsets::MD>,
    /// "9 Nov 2026".
    day_in_year: DateTimeFormatter<fieldsets::YMD>,
    /// "9 November 2026".
    long_day: DateTimeFormatter<fieldsets::YMD>,
    /// "Monday, 9 November 2026 at 20:34".
    full: DateTimeFormatter<fieldsets::YMDET>,
    weekday: DateTimeFormatter<fieldsets::E>,
}

impl Dates {
    /// Formats for the system's region and time zone, with the settings' clock and order.
    pub fn system(clock: Clock, order: DateFormat) -> Self {
        let locale = system_locale_tags()
            .find_map(|tag| parse_locale(&tag))
            .unwrap_or(icu_locale_core::locale!("en"));
        Self::new(&locale, TimeZone::system(), clock, order)
    }

    /// Formats for `locale` in `zone`.
    pub fn new(locale: &Locale, zone: TimeZone, clock: Clock, order: DateFormat) -> Self {
        let mut prefs = icu_datetime::DateTimeFormatterPreferences::from(locale);
        prefs.hour_cycle = match clock {
            Clock::System => None,
            Clock::H24 => Some(HourCycle::H23),
            Clock::H12 => Some(HourCycle::H12),
        };
        // A chosen order puts the parts together here, with the year and day of the
        // Gregorian calendar, the only one the order makes sense in.
        if order != DateFormat::System {
            prefs.calendar_algorithm = Some(CalendarAlgorithm::Gregory);
        }
        // The compiled data covers every locale, falling back to a parent locale's.
        macro_rules! formatter {
            ($field_set:expr) => {
                DateTimeFormatter::try_new(prefs, $field_set)
                    .expect("compiled data covers every locale")
            };
        }
        Self {
            zone,
            order,
            time: formatter!(fieldsets::T::hm()),
            day: formatter!(fieldsets::MD::medium()),
            day_in_year: formatter!(fieldsets::YMD::medium()),
            long_day: formatter!(fieldsets::YMD::long()),
            full: formatter!(fieldsets::YMDE::long().with_time_hm()),
            weekday: formatter!(fieldsets::E::long()),
        }
    }

    fn local(&self, ms: i64) -> civil::DateTime {
        let at = Timestamp::from_millisecond(ms).unwrap_or(Timestamp::UNIX_EPOCH);
        self.zone.to_datetime(at)
    }

    fn now(&self) -> civil::DateTime {
        self.zone.to_datetime(Timestamp::now())
    }

    /// Today's date with its year, to show what this order looks like in the settings.
    pub fn today(&self) -> String {
        self.date(self.now(), false, true)
    }

    /// For the article list: the time for today, a short date otherwise.
    pub fn list(&self, ms: i64) -> String {
        self.list_at(ms, self.now())
    }

    fn list_at(&self, ms: i64, now: civil::DateTime) -> String {
        let d = self.local(ms);
        if d.date() == now.date() {
            self.time.format(&icu(d)).to_string()
        } else {
            self.short_date(d, now)
        }
    }

    /// For the last sync: "Today at 20:34", "Yesterday at 09:12" or "12 Oct at 09:12".
    pub fn synced(&self, ms: i64) -> String {
        self.synced_at(ms, self.now())
    }

    fn synced_at(&self, ms: i64, now: civil::DateTime) -> String {
        let d = self.local(ms);
        let time = self.time.format(&icu(d)).to_string();
        let yesterday = now.date().yesterday().ok();
        if d.date() == now.date() {
            t("format.today").replace("{time}", &time)
        } else if Some(d.date()) == yesterday {
            t("format.yesterday").replace("{time}", &time)
        } else {
            t("format.dayAt")
                .replace("{date}", &self.short_date(d, now))
                .replace("{time}", &time)
        }
    }

    /// For the article's header: weekday, date and time, such as
    /// "Monday, 9 November 2026 at 20:34".
    pub fn full(&self, ms: i64) -> String {
        let d = self.local(ms);
        if self.order == DateFormat::System {
            return self.full.format(&icu(d)).to_string();
        }
        let comma = if self.order == DateFormat::MonthDay {
            ","
        } else {
            ""
        };
        format!(
            "{}{comma} {}, {}",
            self.weekday.format(&icu(d)),
            self.date(d, true, true),
            self.time.format(&icu(d)),
        )
    }

    /// "12 Oct", with the year when it isn't this year's.
    fn short_date(&self, d: civil::DateTime, now: civil::DateTime) -> String {
        self.date(d, false, d.year() != now.year())
    }

    /// The date in the settings' order: "9 Nov 2026", "Nov 9, 2026" or "2026-11-09",
    /// with the month's long name when `long`. ISO always has the year. A region that
    /// names months by number gets its own format instead.
    fn date(&self, d: civil::DateTime, long: bool, with_year: bool) -> String {
        let input = icu(d);
        if self.order == DateFormat::Iso {
            return format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day());
        }
        let named = match self.order {
            DateFormat::System => None,
            _ => self.named_parts(&input, long),
        };
        let Some(parts) = named else {
            let f = if long {
                &self.long_day
            } else if with_year {
                &self.day_in_year
            } else {
                return self.day.format(&input).to_string();
            };
            return f.format(&input).to_string();
        };
        let (day, month, year) = (
            parts.get(parts::DAY),
            parts.get(parts::MONTH),
            parts.get(parts::YEAR),
        );
        let day_month = if self.order == DateFormat::DayMonth {
            format!("{day} {month}")
        } else {
            format!("{month} {day}")
        };
        match (with_year, self.order) {
            (false, _) => day_month,
            (true, DateFormat::DayMonth) => format!("{day_month} {year}"),
            (true, _) => format!("{day_month}, {year}"),
        }
    }

    /// The date's parts, with the month as a name, in the form a date uses: short when
    /// the region has one, long when only that is a name (Finnish, Czech). `None` when
    /// the region names months by number (Japanese, Chinese).
    fn named_parts(&self, input: &DateTime<icu_calendar::Iso>, long: bool) -> Option<Parts> {
        let formatters = if long {
            vec![&self.long_day]
        } else {
            vec![&self.day_in_year, &self.long_day]
        };
        formatters.into_iter().find_map(|f| {
            let mut parts = Parts::default();
            f.format(input).write_to_parts(&mut parts).ok()?;
            parts
                .get(parts::MONTH)
                .chars()
                .any(char::is_alphabetic)
                .then_some(parts)
        })
    }
}

/// A formatted text with the spans of its parts, as `Intl`'s `formatToParts` gives them.
#[derive(Debug, Default)]
struct Parts {
    text: String,
    spans: Vec<(Part, Range<usize>)>,
}

impl Parts {
    /// The text of `part`, empty without one.
    fn get(&self, part: Part) -> &str {
        self.spans
            .iter()
            .find(|(p, _)| *p == part)
            .map_or("", |(_, span)| &self.text[span.clone()])
    }
}

impl fmt::Write for Parts {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.text.push_str(s);
        Ok(())
    }
}

impl PartsWrite for Parts {
    type SubPartsWrite = Self;

    fn with_part(
        &mut self,
        part: Part,
        mut f: impl FnMut(&mut Self) -> fmt::Result,
    ) -> fmt::Result {
        let start = self.text.len();
        f(self)?;
        self.spans.push((part, start..self.text.len()));
        Ok(())
    }
}

/// The system's locale tags, the first that parses winning. On Unix the time format's
/// own variables come first: the UI language, which sys-locale reads, may differ.
fn system_locale_tags() -> impl Iterator<Item = String> {
    let time = if cfg!(all(unix, not(target_os = "macos"))) {
        ["LC_ALL", "LC_TIME"]
            .into_iter()
            .filter_map(|var| std::env::var(var).ok())
            .collect()
    } else {
        Vec::new()
    };
    time.into_iter().chain(sys_locale::get_locale())
}

/// A system locale tag as a locale: `sv_SE.UTF-8` (POSIX) as well as `sv-SE`.
fn parse_locale(tag: &str) -> Option<Locale> {
    let tag = tag.split(['.', '@']).next()?.replace('_', "-");
    Locale::try_from_str(&tag).ok()
}

/// A civil date-time as ICU4X's input.
fn icu(d: civil::DateTime) -> DateTime<icu_calendar::Iso> {
    let date = Date::try_new_iso(
        i32::from(d.year()),
        d.month().unsigned_abs(),
        d.day().unsigned_abs(),
    )
    .unwrap_or_else(|_| Date::try_new_iso(1970, 1, 1).expect("a valid date"));
    let time = Time::try_new(d.hour().unsigned_abs(), d.minute().unsigned_abs(), 0, 0)
        .unwrap_or_else(|_| Time::start_of_day());
    DateTime { date, time }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dates(locale: &str, clock: Clock, order: DateFormat) -> Dates {
        let locale = Locale::try_from_str(locale).unwrap();
        Dates::new(
            &locale,
            TimeZone::fixed(jiff::tz::Offset::constant(1)),
            clock,
            order,
        )
    }

    /// 2026-11-09 20:34 at UTC+1.
    const MS: i64 = 1_794_252_840_000;

    fn at(s: &str) -> civil::DateTime {
        s.parse().unwrap()
    }

    #[test]
    fn local_time_follows_the_zone_and_the_regions_clock() {
        let uk = dates("en-GB", Clock::System, DateFormat::System);
        assert_eq!(uk.list_at(MS, at("2026-11-09T23:00")), "20:34");
        let us = dates("en-US", Clock::System, DateFormat::System);
        assert_eq!(us.list_at(MS, at("2026-11-09T23:00")), "8:34\u{202f}PM");
        let us24 = dates("en-US", Clock::H24, DateFormat::System);
        assert_eq!(us24.list_at(MS, at("2026-11-09T23:00")), "20:34");
    }

    #[test]
    fn other_days_show_a_short_date_with_the_year_only_when_needed() {
        let uk = dates("en-GB", Clock::System, DateFormat::System);
        assert_eq!(uk.list_at(MS, at("2026-11-12T08:00")), "9 Nov");
        assert_eq!(uk.list_at(MS, at("2027-01-02T08:00")), "9 Nov 2026");
        let sv = dates("sv-SE", Clock::System, DateFormat::System);
        assert_eq!(sv.list_at(MS, at("2026-11-12T08:00")), "9 nov.");
    }

    #[test]
    fn a_chosen_order_keeps_the_regions_month_names() {
        let sv = dates("sv-SE", Clock::System, DateFormat::MonthDay);
        assert_eq!(sv.list_at(MS, at("2027-01-02T08:00")), "nov. 9, 2026");
        let iso = dates("en-US", Clock::System, DateFormat::Iso);
        assert_eq!(iso.list_at(MS, at("2026-11-12T08:00")), "2026-11-09");
    }

    #[test]
    fn the_sync_line_says_today_or_yesterday() {
        let uk = dates("en-GB", Clock::System, DateFormat::System);
        assert_eq!(uk.synced_at(MS, at("2026-11-09T23:00")), "Today at 20:34");
        assert_eq!(
            uk.synced_at(MS, at("2026-11-10T09:00")),
            "Yesterday at 20:34"
        );
        assert_eq!(uk.synced_at(MS, at("2026-11-12T09:00")), "9 Nov at 20:34");
    }

    #[test]
    fn the_header_has_the_weekday_and_full_date() {
        let uk = dates("en-GB", Clock::System, DateFormat::System);
        assert_eq!(uk.full(MS), "Monday, 9 November 2026 at 20:34");
        let ordered = dates("en-GB", Clock::System, DateFormat::MonthDay);
        assert_eq!(ordered.full(MS), "Monday, November 9, 2026, 20:34");
    }

    #[test]
    fn a_chosen_order_takes_the_month_as_dates_have_it() {
        // Czech and Russian name the month differently inside a date.
        let cs = dates("cs-CZ", Clock::System, DateFormat::DayMonth);
        assert_eq!(cs.date(cs.local(MS), true, true), "9 listopadu 2026");
        let ru = dates("ru-RU", Clock::System, DateFormat::DayMonth);
        assert!(ru.date(ru.local(MS), true, false).contains("ноября"));
        // Japanese names months by number: its own format.
        let ja = dates("ja-JP", Clock::System, DateFormat::MonthDay);
        assert_eq!(
            ja.date(ja.local(MS), false, true),
            dates("ja-JP", Clock::System, DateFormat::System).date(ja.local(MS), false, true)
        );
    }

    #[test]
    fn a_chosen_order_keeps_to_the_gregorian_calendar() {
        // Persian's own calendar would give another month and year.
        let fa = dates("fa-IR", Clock::System, DateFormat::DayMonth);
        let date = fa.date(fa.local(MS), true, true);
        assert!(date.contains("نوامبر"), "{date}");
    }

    #[test]
    fn posix_locale_tags_parse() {
        assert_eq!(parse_locale("sv_SE.UTF-8").unwrap().to_string(), "sv-SE");
        assert_eq!(parse_locale("en-US").unwrap().to_string(), "en-US");
    }
}
