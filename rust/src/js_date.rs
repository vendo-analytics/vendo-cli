//! `Date.parse` / `new Date(string)` as V8 (Node) evaluates them, and
//! `Date.prototype.toISOString` (VE-3727). A port of V8's `DateParser`
//! (`src/date/dateparser*`): an ES5 ISO pass, then the legacy parser for
//! whatever is left, so `2026-07-01` is UTC midnight, `2026-07-01T10:00` and
//! `July 1, 2026` are local time, `2026-02-30` rolls over to March 2 and
//! `20260701` is invalid. Local times go through the TZ database (jiff) with
//! V8's rule for gaps and folds: the offset in force before the transition.

use jiff::tz::TimeZone;

/// V8's `kNone`: a component that was not given.
const NONE: i64 = i32::MAX as i64;
/// V8 reads at most nine significant digits of a numeral.
const MAX_SIGNIFICANT_DIGITS: usize = 9;
/// Largest Smi on 64-bit Node (no pointer compression), which bounds offsets.
const SMI_MAX: u32 = i32::MAX as u32;
const MS_PER_DAY: i64 = 86_400_000;
/// Days in a 400-year Gregorian cycle.
const DAYS_PER_400_YEARS: i64 = 146_097;
/// ±8.64e15 ms: the range of a JavaScript time value.
const MAX_TIME_MS: i64 = 8_640_000_000_000_000;
/// Local times may sit up to 10 days beyond it before conversion to UTC.
const MAX_TIME_BEFORE_UTC_MS: i64 = MAX_TIME_MS + 864_000_000;

/// `Date.parse(value)` in the system time zone: milliseconds since the epoch,
/// `None` where JavaScript gives NaN.
pub fn parse(value: &str) -> Option<i64> {
    parse_in(value, &TimeZone::system())
}

/// [`parse`] with an explicit zone for local times.
pub fn parse_in(value: &str, tz: &TimeZone) -> Option<i64> {
    let Fields { year, month, day, hour, minute, second, millisecond, utc_offset } = parse_fields(value)?;
    let date = make_day(year, month, day)? * MS_PER_DAY + make_time(hour, minute, second, millisecond);
    let utc = match utc_offset {
        None => {
            if !(-MAX_TIME_BEFORE_UTC_MS..=MAX_TIME_BEFORE_UTC_MS).contains(&date) {
                return None;
            }
            local_to_utc(date, tz)?
        }
        Some(offset_seconds) => {
            let utc = date - offset_seconds * 1000;
            if !(-MAX_TIME_MS..=MAX_TIME_MS).contains(&utc) {
                return None;
            }
            utc
        }
    };
    (-MAX_TIME_MS..=MAX_TIME_MS).contains(&utc).then_some(utc)
}

/// `Date.prototype.toISOString()`: `YYYY-MM-DDTHH:mm:ss.sssZ`, with a signed
/// six-digit year outside 0000–9999. `"Invalid Date"` outside the JS range.
pub fn iso_string(ms: i64) -> String {
    if !(-MAX_TIME_MS..=MAX_TIME_MS).contains(&ms) {
        return "Invalid Date".to_string();
    }
    let (days, in_day) = (ms.div_euclid(MS_PER_DAY), ms.rem_euclid(MS_PER_DAY));
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second, millis) = (in_day / 3_600_000, in_day / 60_000 % 60, in_day / 1000 % 60, in_day % 1000);
    let year = match year {
        0..=9999 => format!("{year:04}"),
        y if y < 0 => format!("-{:06}", -y),
        y => format!("+{y:06}"),
    };
    format!("{year}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

// ── V8's parser ────────────────────────────────────────────────────────────

/// What `DateParser::Parse` writes: month is 0-based, `utc_offset` is in
/// seconds and `None` for local time.
struct Fields {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    millisecond: i64,
    utc_offset: Option<i64>,
}

fn parse_fields(value: &str) -> Option<Fields> {
    let mut scanner = Tokenizer::new(value);
    let mut day = DayComposer::default();
    let mut time = TimeComposer::default();
    let mut tz = TimeZoneComposer::default();

    let unhandled = parse_es5_date_time(&mut scanner, &mut day, &mut time, &mut tz);
    if matches!(unhandled, Token::Invalid) {
        return None;
    }
    let mut has_read_number = !day.is_empty();
    let mut token = unhandled;
    while !matches!(token, Token::EndOfInput) {
        match token {
            Token::Number { value: n, .. } => {
                has_read_number = true;
                if scanner.skip_symbol(':') {
                    if scanner.skip_symbol(':') {
                        // n + "::"
                        if !time.is_empty() {
                            return None;
                        }
                        time.add(n);
                        time.add(0);
                    } else {
                        // n + ":"
                        if !time.add(n) {
                            return None;
                        }
                        if scanner.peek().is_symbol('.') {
                            scanner.next();
                        }
                    }
                } else if scanner.skip_symbol('.') && time.is_expecting(n) {
                    time.add(n);
                    let next = scanner.peek();
                    if !next.is_number() {
                        return None;
                    }
                    scanner.next();
                    time.add_final(read_milliseconds(next));
                } else if tz.is_expecting(n) {
                    tz.minute = n;
                } else if time.is_expecting(n) {
                    time.add_final(n);
                    // Only the end, white space, "Z" or a sign may follow a finished time.
                    let peek = scanner.peek();
                    if !matches!(peek, Token::EndOfInput | Token::WhiteSpace) && !peek.is_keyword_z() && !peek.is_sign()
                    {
                        return None;
                    }
                } else {
                    if !day.add(n) {
                        return None;
                    }
                    scanner.skip_symbol('-');
                }
            }
            Token::Keyword { kind, value, .. } => {
                if kind == Keyword::AmPm && !time.is_empty() {
                    time.hour_offset = Some(value);
                } else if kind == Keyword::MonthName {
                    day.named_month = Some(value);
                    scanner.skip_symbol('-');
                } else if kind == Keyword::TimeZoneName && has_read_number {
                    tz.set(value);
                } else {
                    // Garbage words are illegal once a number has been read, and
                    // must be separated from the first number.
                    if has_read_number || scanner.peek().is_number() {
                        return None;
                    }
                }
            }
            Token::Symbol(sign @ ('+' | '-')) if tz.is_utc() || !time.is_empty() => {
                // A UTC offset, only after "UTC"/"GMT" or a time.
                tz.sign = if sign == '+' { 1 } else { -1 };
                let (mut n, mut length) = (0, 0);
                if let Token::Number { value, length: len } = scanner.peek() {
                    scanner.next();
                    (n, length) = (value, len);
                }
                has_read_number = true;
                if scanner.peek().is_symbol(':') {
                    tz.hour = n;
                    tz.minute = NONE;
                } else if length == 1 || length == 2 {
                    tz.hour = n;
                    tz.minute = 0;
                } else if length == 3 || length == 4 {
                    tz.hour = n / 100;
                    tz.minute = n % 100;
                } else {
                    return None;
                }
            }
            Token::Symbol('+' | '-' | ')') if has_read_number => return None,
            _ => {}
        }
        token = scanner.next();
    }

    let (year, month, day) = day.write()?;
    let [hour, minute, second, millisecond] = time.write()?;
    let utc_offset = tz.write()?;
    Some(Fields { year, month, day, hour, minute, second, millisecond, utc_offset })
}

/// `[('-'|'+')yy]yyyy[-MM[-DD]][THH:mm[:ss[.sss]][Z|(+|-)hh:mm]]`. Returns
/// the first token it did not use, `EndOfInput` when the whole string was an
/// ES5 date, or `Invalid` when the time part is malformed.
fn parse_es5_date_time(
    scanner: &mut Tokenizer,
    day: &mut DayComposer,
    time: &mut TimeComposer,
    tz: &mut TimeZoneComposer,
) -> Token {
    if scanner.peek().is_sign() {
        // Keep the sign token, so invalid dates can be detected later.
        let sign_token = scanner.next();
        if !scanner.peek().is_fixed_length_number(6) {
            return sign_token;
        }
        let year = scanner.next().number();
        let negative = sign_token.is_symbol('-');
        if negative && year == 0 {
            return sign_token;
        }
        day.add(if negative { -year } else { year });
    } else if scanner.peek().is_fixed_length_number(4) {
        let year = scanner.next().number();
        day.add(year);
    } else {
        return scanner.next();
    }
    if scanner.skip_symbol('-') {
        if !scanner.peek().is_fixed_length_number(2) || !is_month(scanner.peek().number()) {
            return scanner.next();
        }
        let month = scanner.next().number();
        day.add(month);
        if scanner.skip_symbol('-') {
            if !scanner.peek().is_fixed_length_number(2) || !is_day(scanner.peek().number()) {
                return scanner.next();
            }
            let d = scanner.next().number();
            day.add(d);
        }
    }
    if !scanner.peek().is_keyword(Keyword::TimeSeparator) {
        if !matches!(scanner.peek(), Token::EndOfInput) {
            return scanner.next();
        }
    } else {
        scanner.next();
        if !scanner.peek().is_fixed_length_number(2) || !(0..=24).contains(&scanner.peek().number()) {
            return Token::Invalid;
        }
        // 24:00[:00[.000]] is midnight at the end of the day; nothing else starts with 24.
        let hour_is_24 = scanner.peek().number() == 24;
        let hour = scanner.next().number();
        time.add(hour);
        if !scanner.skip_symbol(':') {
            return Token::Invalid;
        }
        let peek = scanner.peek();
        if !peek.is_fixed_length_number(2) || !is_minute(peek.number()) || (hour_is_24 && peek.number() > 0) {
            return Token::Invalid;
        }
        scanner.next();
        time.add(peek.number());
        if scanner.skip_symbol(':') {
            let peek = scanner.peek();
            if !peek.is_fixed_length_number(2) || !is_second(peek.number()) || (hour_is_24 && peek.number() > 0) {
                return Token::Invalid;
            }
            scanner.next();
            time.add(peek.number());
            if scanner.skip_symbol('.') {
                let peek = scanner.peek();
                if !peek.is_number() || (hour_is_24 && peek.number() > 0) {
                    return Token::Invalid;
                }
                scanner.next();
                // More or fewer than three digits are allowed.
                time.add(read_milliseconds(peek));
            }
        }
        if scanner.peek().is_keyword_z() {
            scanner.next();
            tz.set(0);
        } else if scanner.peek().is_sign() {
            tz.sign = if scanner.next().is_symbol('+') { 1 } else { -1 };
            if scanner.peek().is_fixed_length_number(4) {
                // hhmm extension syntax.
                let hour_minute = scanner.next().number();
                let (hour, minute) = (hour_minute / 100, hour_minute % 100);
                if !is_hour(hour) || !is_minute(minute) {
                    return Token::Invalid;
                }
                tz.hour = hour;
                tz.minute = minute;
            } else {
                if !scanner.peek().is_fixed_length_number(2) || !is_hour(scanner.peek().number()) {
                    return Token::Invalid;
                }
                tz.hour = scanner.next().number();
                if !scanner.skip_symbol(':') {
                    return Token::Invalid;
                }
                if !scanner.peek().is_fixed_length_number(2) || !is_minute(scanner.peek().number()) {
                    return Token::Invalid;
                }
                tz.minute = scanner.next().number();
            }
        }
        if !matches!(scanner.peek(), Token::EndOfInput) {
            return Token::Invalid;
        }
    }
    // Without an offset, date-only forms are UTC and date-time forms local.
    if tz.is_empty() && time.is_empty() {
        tz.set(0);
    }
    day.iso = true;
    Token::EndOfInput
}

/// The first three digits of a fraction as milliseconds, inferred from the
/// numeral's value and length (leading zeros count towards the length).
fn read_milliseconds(token: Token) -> i64 {
    let Token::Number { value, length } = token else { return 0 };
    match length {
        1 => value * 100,
        2 => value * 10,
        3 => value,
        _ => value / 10_i64.pow(length.min(MAX_SIGNIFICANT_DIGITS) as u32 - 3),
    }
}

fn is_month(n: i64) -> bool {
    (1..=12).contains(&n)
}
fn is_day(n: i64) -> bool {
    (1..=31).contains(&n)
}
fn is_hour(n: i64) -> bool {
    (0..=23).contains(&n)
}
fn is_minute(n: i64) -> bool {
    (0..=59).contains(&n)
}
fn is_second(n: i64) -> bool {
    (0..=59).contains(&n)
}
fn is_millisecond(n: i64) -> bool {
    (0..=999).contains(&n)
}

#[derive(Default)]
struct DayComposer {
    comp: [i64; 3],
    index: usize,
    named_month: Option<i64>,
    /// Set after a complete ES5 date: always year-month-day order.
    iso: bool,
}

impl DayComposer {
    fn is_empty(&self) -> bool {
        self.index == 0
    }

    fn add(&mut self, n: i64) -> bool {
        if self.index < 3 {
            self.comp[self.index] = n;
            self.index += 1;
            true
        } else {
            false
        }
    }

    /// Year, 0-based month and day. Missing parts default to 1 (so a missing
    /// year is 1 → 2001, as in V8).
    fn write(mut self) -> Option<(i64, i64, i64)> {
        if self.index < 1 {
            return None;
        }
        while self.index < 3 {
            self.comp[self.index] = 1;
            self.index += 1;
        }
        let [c0, c1, c2] = self.comp;
        let (mut year, month, day) = match self.named_month {
            None if self.iso || !is_day(c0) => (c0, c1, c2),
            None => (c2, c0, c1),
            Some(month) if !is_day(c0) => (c0, month, c1),
            Some(month) => (c1, month, c0),
        };
        if !self.iso {
            match year {
                0..=49 => year += 2000,
                50..=99 => year += 1900,
                _ => {}
            }
        }
        (is_month(month) && is_day(day)).then_some((year, month - 1, day))
    }
}

#[derive(Default)]
struct TimeComposer {
    comp: [i64; 4],
    index: usize,
    hour_offset: Option<i64>,
}

impl TimeComposer {
    fn is_empty(&self) -> bool {
        self.index == 0
    }

    fn is_expecting(&self, n: i64) -> bool {
        (self.index == 1 && is_minute(n)) || (self.index == 2 && is_second(n)) || (self.index == 3 && is_millisecond(n))
    }

    fn add(&mut self, n: i64) -> bool {
        if self.index < 4 {
            self.comp[self.index] = n;
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn add_final(&mut self, n: i64) -> bool {
        if !self.add(n) {
            return false;
        }
        while self.index < 4 {
            self.comp[self.index] = 0;
            self.index += 1;
        }
        true
    }

    fn write(mut self) -> Option<[i64; 4]> {
        while self.index < 4 {
            self.comp[self.index] = 0;
            self.index += 1;
        }
        let [mut hour, minute, second, millisecond] = self.comp;
        if let Some(offset) = self.hour_offset {
            if !(0..=12).contains(&hour) {
                return None;
            }
            hour = hour % 12 + offset;
        }
        let valid = is_hour(hour) && is_minute(minute) && is_second(second) && is_millisecond(millisecond);
        // Hour 24 is allowed when everything after it is zero.
        if !valid && (hour != 24 || minute != 0 || second != 0 || millisecond != 0) {
            return None;
        }
        Some([hour, minute, second, millisecond])
    }
}

struct TimeZoneComposer {
    sign: i64,
    hour: i64,
    minute: i64,
}

impl Default for TimeZoneComposer {
    fn default() -> Self {
        TimeZoneComposer { sign: NONE, hour: NONE, minute: NONE }
    }
}

impl TimeZoneComposer {
    fn set(&mut self, offset_hours: i64) {
        self.sign = if offset_hours < 0 { -1 } else { 1 };
        self.hour = offset_hours.abs();
        self.minute = 0;
    }

    fn is_expecting(&self, n: i64) -> bool {
        self.hour != NONE && self.minute == NONE && is_minute(n)
    }

    fn is_utc(&self) -> bool {
        self.hour == 0 && self.minute == 0
    }

    fn is_empty(&self) -> bool {
        self.hour == NONE
    }

    /// The offset in seconds, `Some(None)` for local time, `None` when it
    /// doesn't fit V8's Smi.
    fn write(self) -> Option<Option<i64>> {
        if self.sign == NONE {
            return Some(None);
        }
        let hour = if self.hour == NONE { 0 } else { self.hour };
        let minute = if self.minute == NONE { 0 } else { self.minute };
        // V8 does this in unsigned 32-bit arithmetic.
        let total = (hour as u32).wrapping_mul(3600).wrapping_add((minute as u32).wrapping_mul(60));
        if total > SMI_MAX {
            return None;
        }
        Some(Some(if self.sign < 0 { -i64::from(total) } else { i64::from(total) }))
    }
}

// ── tokens ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Keyword {
    Invalid,
    MonthName,
    TimeZoneName,
    TimeSeparator,
    AmPm,
}

#[derive(Clone, Copy, Debug)]
enum Token {
    Invalid,
    Unknown,
    WhiteSpace,
    EndOfInput,
    Number { value: i64, length: usize },
    Symbol(char),
    Keyword { kind: Keyword, value: i64, length: usize },
}

impl Token {
    fn is_number(&self) -> bool {
        matches!(self, Token::Number { .. })
    }

    fn number(&self) -> i64 {
        match self {
            Token::Number { value, .. } => *value,
            _ => 0,
        }
    }

    fn is_fixed_length_number(&self, n: usize) -> bool {
        matches!(self, Token::Number { length, .. } if *length == n)
    }

    fn is_symbol(&self, c: char) -> bool {
        matches!(self, Token::Symbol(s) if *s == c)
    }

    fn is_sign(&self) -> bool {
        self.is_symbol('+') || self.is_symbol('-')
    }

    fn is_keyword(&self, kind: Keyword) -> bool {
        matches!(self, Token::Keyword { kind: k, .. } if *k == kind)
    }

    fn is_keyword_z(&self) -> bool {
        matches!(self, Token::Keyword { kind: Keyword::TimeZoneName, value: 0, length: 1 })
    }
}

/// The keyword table: words match on their first three letters, and words
/// longer than three letters only for month names.
const KEYWORDS: &[(&[u8; 3], Keyword, i64)] = &[
    (b"jan", Keyword::MonthName, 1),
    (b"feb", Keyword::MonthName, 2),
    (b"mar", Keyword::MonthName, 3),
    (b"apr", Keyword::MonthName, 4),
    (b"may", Keyword::MonthName, 5),
    (b"jun", Keyword::MonthName, 6),
    (b"jul", Keyword::MonthName, 7),
    (b"aug", Keyword::MonthName, 8),
    (b"sep", Keyword::MonthName, 9),
    (b"oct", Keyword::MonthName, 10),
    (b"nov", Keyword::MonthName, 11),
    (b"dec", Keyword::MonthName, 12),
    (b"am\0", Keyword::AmPm, 0),
    (b"pm\0", Keyword::AmPm, 12),
    (b"ut\0", Keyword::TimeZoneName, 0),
    (b"utc", Keyword::TimeZoneName, 0),
    (b"z\0\0", Keyword::TimeZoneName, 0),
    (b"gmt", Keyword::TimeZoneName, 0),
    (b"cdt", Keyword::TimeZoneName, -5),
    (b"cst", Keyword::TimeZoneName, -6),
    (b"edt", Keyword::TimeZoneName, -4),
    (b"est", Keyword::TimeZoneName, -5),
    (b"mdt", Keyword::TimeZoneName, -6),
    (b"mst", Keyword::TimeZoneName, -7),
    (b"pdt", Keyword::TimeZoneName, -7),
    (b"pst", Keyword::TimeZoneName, -8),
    (b"t\0\0", Keyword::TimeSeparator, 0),
];

fn lookup_keyword(prefix: [u32; 3], length: usize) -> (Keyword, i64) {
    KEYWORDS
        .iter()
        .find(|(word, kind, _)| {
            word.iter().zip(prefix).all(|(w, p)| u32::from(*w) == p) && (length <= 3 || *kind == Keyword::MonthName)
        })
        .map_or((Keyword::Invalid, 0), |(_, kind, value)| (*kind, *value))
}

/// ECMAScript WhiteSpace (not line terminators).
fn is_white_space(c: u32) -> bool {
    matches!(c, 0x09 | 0x0B | 0x0C | 0x20 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000 | 0xFEFF)
}

fn is_line_terminator(c: u32) -> bool {
    matches!(c, 0x0A | 0x0D | 0x2028 | 0x2029)
}

/// V8's `InputReader` over UTF-16 code units; a NUL ends the input.
struct Reader {
    units: Vec<u32>,
    index: usize,
    ch: u32,
}

impl Reader {
    fn new(value: &str) -> Self {
        let mut reader = Reader { units: value.encode_utf16().map(u32::from).collect(), index: 0, ch: 0 };
        reader.advance();
        reader
    }

    fn advance(&mut self) {
        self.ch = self.units.get(self.index).copied().unwrap_or(0);
        self.index += 1;
    }

    fn is_digit(&self) -> bool {
        (u32::from(b'0')..=u32::from(b'9')).contains(&self.ch)
    }

    /// At most nine significant digits count; the rest are skipped.
    fn read_unsigned_numeral(&mut self) -> i64 {
        while self.ch == u32::from(b'0') {
            self.advance();
        }
        let (mut n, mut digits) = (0i64, 0usize);
        while self.is_digit() {
            if digits < MAX_SIGNIFICANT_DIGITS {
                n = n * 10 + i64::from(self.ch - u32::from(b'0'));
            }
            digits += 1;
            self.advance();
        }
        n
    }

    /// A word (characters >= 'A' that aren't white space): its lower-cased
    /// three-character prefix, zero-padded, and its length.
    fn read_word(&mut self) -> ([u32; 3], usize) {
        let mut prefix = [0u32; 3];
        let mut length = 0;
        while self.ch >= u32::from(b'A') && !is_white_space(self.ch) {
            if length < 3 {
                prefix[length] = self.ch | 0x20;
            }
            length += 1;
            self.advance();
        }
        (prefix, length)
    }

    fn skip(&mut self, c: char) -> bool {
        if self.ch == c as u32 {
            self.advance();
            true
        } else {
            false
        }
    }

    fn skip_parentheses(&mut self) -> bool {
        if self.ch != u32::from(b'(') {
            return false;
        }
        let mut balance = 0;
        loop {
            if self.ch == u32::from(b')') {
                balance -= 1;
            } else if self.ch == u32::from(b'(') {
                balance += 1;
            }
            self.advance();
            if balance <= 0 || self.ch == 0 {
                return true;
            }
        }
    }

    fn scan(&mut self) -> Token {
        let start = self.index;
        if self.ch == 0 {
            return Token::EndOfInput;
        }
        if self.is_digit() {
            let value = self.read_unsigned_numeral();
            return Token::Number { value, length: self.index - start };
        }
        for symbol in [':', '-', '+', '.', ')'] {
            if self.skip(symbol) {
                return Token::Symbol(symbol);
            }
        }
        if self.ch >= u32::from(b'A') && !is_white_space(self.ch) {
            let (prefix, length) = self.read_word();
            let (kind, value) = lookup_keyword(prefix, length);
            return Token::Keyword { kind, value, length };
        }
        if is_white_space(self.ch) || is_line_terminator(self.ch) {
            self.advance();
            return Token::WhiteSpace;
        }
        if self.skip_parentheses() {
            return Token::Unknown;
        }
        self.advance();
        Token::Unknown
    }
}

/// One token of look-ahead, as V8's `DateStringTokenizer`.
struct Tokenizer {
    reader: Reader,
    next: Token,
}

impl Tokenizer {
    fn new(value: &str) -> Self {
        let mut reader = Reader::new(value);
        let next = reader.scan();
        Tokenizer { reader, next }
    }

    fn next(&mut self) -> Token {
        let token = self.next;
        self.next = self.reader.scan();
        token
    }

    fn peek(&self) -> Token {
        self.next
    }

    fn skip_symbol(&mut self, c: char) -> bool {
        if self.next.is_symbol(c) {
            self.next = self.reader.scan();
            true
        } else {
            false
        }
    }
}

// ── time values ────────────────────────────────────────────────────────────

/// ES `MakeDay` (days since the epoch); V8 gives up beyond ±1,000,000 years.
fn make_day(year: i64, month: i64, date: i64) -> Option<i64> {
    if !(-1_000_000..=1_000_000).contains(&year) || !(-10_000_000..=10_000_000).contains(&month) {
        return None;
    }
    let (mut y, mut m) = (year + month / 12, month % 12);
    if m < 0 {
        m += 12;
        y -= 1;
    }
    Some(days_from_civil(y, m + 1, 1) + date - 1)
}

fn make_time(hour: i64, minute: i64, second: i64, millisecond: i64) -> i64 {
    hour * 3_600_000 + minute * 60_000 + second * 1000 + millisecond
}

/// Days since 1970-01-01 of a proleptic Gregorian date (month 1–12).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse of [`days_from_civil`]: (year, month 1–12, day).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Wall-clock milliseconds in `tz` to UTC. In a gap or a fold the offset in
/// force before the transition applies, as V8 does (jiff's "compatible").
fn local_to_utc(local_ms: i64, tz: &TimeZone) -> Option<i64> {
    let (days, in_day) = (local_ms.div_euclid(MS_PER_DAY), local_ms.rem_euclid(MS_PER_DAY));
    let (year, month, day) = civil_from_days(days);
    // jiff covers years ±9999. Beyond that the zone's rules repeat with the
    // 400-year Gregorian cycle, so look the offset up whole cycles closer.
    let cycles = if year > 9000 {
        (year - 9000 + 399) / 400
    } else if year < -9000 {
        -((-9000 - year + 399) / 400)
    } else {
        0
    };
    let civil = jiff::civil::DateTime::new(
        i16::try_from(year - cycles * 400).ok()?,
        month as i8,
        day as i8,
        (in_day / 3_600_000) as i8,
        (in_day / 60_000 % 60) as i8,
        (in_day / 1000 % 60) as i8,
        (in_day % 1000 * 1_000_000) as i32,
    )
    .ok()?;
    // The resolved instant, not wall clock minus offset: in a gap the
    // instant's own offset is the one after the transition.
    let instant = civil.to_zoned(tz.clone()).ok()?.timestamp().as_millisecond();
    Some(instant + cycles * DAYS_PER_400_YEARS * MS_PER_DAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_days_round_trip() {
        for days in [-100_000_000, -719_468, -1, 0, 1, 19_000, 2_932_896, 100_000_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{days}");
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2026, 7, 1), 20_635);
    }

    #[test]
    fn iso_strings_match_to_iso_string() {
        assert_eq!(iso_string(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_string(-1), "1969-12-31T23:59:59.999Z");
        assert_eq!(iso_string(1_782_900_000_123), "2026-07-01T10:00:00.123Z");
        assert_eq!(iso_string(MAX_TIME_MS), "+275760-09-13T00:00:00.000Z");
        assert_eq!(iso_string(-MAX_TIME_MS), "-271821-04-20T00:00:00.000Z");
        assert_eq!(iso_string(-62_198_755_200_000), "-000001-01-01T00:00:00.000Z");
        assert_eq!(iso_string(253_402_300_800_000), "+010000-01-01T00:00:00.000Z");
        assert_eq!(iso_string(MAX_TIME_MS + 1), "Invalid Date");
    }

    /// `new Date(s).getTime()` from Node 24.16 under `TZ=Australia/Sydney`,
    /// `TZ=UTC` and `TZ=America/New_York` (scratch script `dates.mjs`, VE-3727).
    const NODE_DATES: &[(&str, [Option<i64>; 3])] = &[
        ("0", [Some(946645200000), Some(946684800000), Some(946702800000)]),
        ("1", [Some(978267600000), Some(978307200000), Some(978325200000)]),
        ("12", [Some(1007125200000), Some(1007164800000), Some(1007182800000)]),
        ("2026", [Some(1767225600000), Some(1767225600000), Some(1767225600000)]),
        ("20260701", [None, None, None]),
        ("2026-07-01", [Some(1782864000000), Some(1782864000000), Some(1782864000000)]),
        ("2026-07", [Some(1782864000000), Some(1782864000000), Some(1782864000000)]),
        ("+002026-07-01", [Some(1782864000000), Some(1782864000000), Some(1782864000000)]),
        ("-000001-01-01", [Some(-62198755200000), Some(-62198755200000), Some(-62198755200000)]),
        ("-000000-01-01", [Some(978267600000), Some(978307200000), Some(978325200000)]),
        ("+275760-09-13", [Some(8640000000000000), Some(8640000000000000), Some(8640000000000000)]),
        ("+275760-09-14", [None, None, None]),
        ("-271821-04-20", [Some(-8640000000000000), Some(-8640000000000000), Some(-8640000000000000)]),
        ("-271821-04-19", [None, None, None]),
        ("0049-01-01", [Some(-60620832000000), Some(-60620832000000), Some(-60620832000000)]),
        ("0000-01-01", [Some(-62167219200000), Some(-62167219200000), Some(-62167219200000)]),
        ("2026-07-01Z", [Some(1782864000000), Some(1782864000000), Some(1782864000000)]),
        ("2026-07-01z", [Some(1782864000000), Some(1782864000000), Some(1782864000000)]),
        ("2026-07-01T10:00", [Some(1782864000000), Some(1782900000000), Some(1782914400000)]),
        ("2026-07-01T10:00:00", [Some(1782864000000), Some(1782900000000), Some(1782914400000)]),
        ("2026-07-01T10:00:00.5", [Some(1782864000500), Some(1782900000500), Some(1782914400500)]),
        ("2026-07-01T10:00:00.123456", [Some(1782864000123), Some(1782900000123), Some(1782914400123)]),
        ("2026-07-01T10:00:00Z", [Some(1782900000000), Some(1782900000000), Some(1782900000000)]),
        ("2026-07-01T10:00:00.000+02:00", [Some(1782892800000), Some(1782892800000), Some(1782892800000)]),
        ("2026-07-01T10:00:00-0500", [Some(1782918000000), Some(1782918000000), Some(1782918000000)]),
        ("2026-07-01T10:00:00+05", [None, None, None]),
        ("2026-07-01T10:00:00+0530", [Some(1782880200000), Some(1782880200000), Some(1782880200000)]),
        ("2026-07-01T10:00:00+05:30", [Some(1782880200000), Some(1782880200000), Some(1782880200000)]),
        ("2026-07-01T10:00:00+24:00", [None, None, None]),
        ("2026-07-01t10:00:00z", [Some(1782900000000), Some(1782900000000), Some(1782900000000)]),
        ("2026-07-01T10:00Z", [Some(1782900000000), Some(1782900000000), Some(1782900000000)]),
        ("2026-07-01T10:00:00.Z", [None, None, None]),
        ("2026-07-01T10:00:00.1234567890123Z", [Some(1782900000123), Some(1782900000123), Some(1782900000123)]),
        ("2026-07-01T10:00:00.0001234567890Z", [Some(1782900000123), Some(1782900000123), Some(1782900000123)]),
        ("2026-07-01T24:00:00Z", [Some(1782950400000), Some(1782950400000), Some(1782950400000)]),
        ("2026-07-01T24:00Z", [Some(1782950400000), Some(1782950400000), Some(1782950400000)]),
        ("2026-07-01T24:00", [Some(1782914400000), Some(1782950400000), Some(1782964800000)]),
        ("2026-07-01T24:00:01Z", [None, None, None]),
        ("2026-07-01T24:00:00.001Z", [None, None, None]),
        ("2026-07-01T23:59:60Z", [None, None, None]),
        ("2026-12-31T24:00:00Z", [Some(1798761600000), Some(1798761600000), Some(1798761600000)]),
        ("2026-07-01T10", [None, None, None]),
        ("2026-07-01T1:00", [None, None, None]),
        ("2026-07-01T", [None, None, None]),
        ("2026-07-01T10:00:00 ", [None, None, None]),
        ("2026-07-01T10:00:00Z ", [None, None, None]),
        ("2026-13-01", [None, None, None]),
        ("2026-00-10", [None, None, None]),
        ("2026-07-00", [None, None, None]),
        ("2026-07-32", [None, None, None]),
        ("2026-02-30", [Some(1772409600000), Some(1772409600000), Some(1772409600000)]),
        ("2026-02-31", [Some(1772496000000), Some(1772496000000), Some(1772496000000)]),
        ("2026-04-31", [Some(1777593600000), Some(1777593600000), Some(1777593600000)]),
        ("2025-02-29", [Some(1740787200000), Some(1740787200000), Some(1740787200000)]),
        ("2024-02-29", [Some(1709164800000), Some(1709164800000), Some(1709164800000)]),
        ("2100-02-29", [Some(4107542400000), Some(4107542400000), Some(4107542400000)]),
        (" 2026-07-01", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("2026-07-01 ", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("  2026-07-01T10:00:00Z", [None, None, None]),
        ("\t2026-07-01", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("2026-07-01\n", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("\u{a0}2026-07-01", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("2026-07-01\u{2028}", [None, None, None]),
        ("2026/07/01", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("2026-7-1", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("2026-7-01", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("07/01/2026", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("7/1/2026", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("July 1, 2026", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("1 Jul 2026", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("Jul 1 2026 10:30", [Some(1782865800000), Some(1782901800000), Some(1782916200000)]),
        (
            "Wed Jul 01 2026 10:00:00 GMT+0000 (Coordinated Universal Time)",
            [Some(1782900000000), Some(1782900000000), Some(1782900000000)],
        ),
        ("Wed, 01 Jul 2026 10:00:00 GMT", [Some(1782900000000), Some(1782900000000), Some(1782900000000)]),
        ("1/2/3", [Some(1041426000000), Some(1041465600000), Some(1041483600000)]),
        ("12/31/99", [Some(946558800000), Some(946598400000), Some(946616400000)]),
        ("7/1", [Some(993909600000), Some(993945600000), Some(993960000000)]),
        ("July 1", [Some(993909600000), Some(993945600000), Some(993960000000)]),
        ("1 July", [Some(993909600000), Some(993945600000), Some(993960000000)]),
        ("2026 July 1", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("July 2026 1", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("2026-07-01 10:00:00", [Some(1782864000000), Some(1782900000000), Some(1782914400000)]),
        ("2026-07-01 10:00:00Z", [Some(1782900000000), Some(1782900000000), Some(1782900000000)]),
        ("2026-07-01 10:00:00 GMT+0200", [Some(1782892800000), Some(1782892800000), Some(1782892800000)]),
        ("2026-07-01 10:00:00 +0200", [Some(1782892800000), Some(1782892800000), Some(1782892800000)]),
        ("2026-07-01 10:00:00 UTC+2", [Some(1782892800000), Some(1782892800000), Some(1782892800000)]),
        ("2026-07-01 10:00:00 UTC+02:30", [Some(1782891000000), Some(1782891000000), Some(1782891000000)]),
        ("2026-07-01 10:00:00 EST", [Some(1782918000000), Some(1782918000000), Some(1782918000000)]),
        ("2026-07-01 10:00:00 PDT", [Some(1782925200000), Some(1782925200000), Some(1782925200000)]),
        ("2026-07-01 10:00:00 GMT-8", [Some(1782928800000), Some(1782928800000), Some(1782928800000)]),
        ("2026-07-01 10:00 PM", [Some(1782907200000), Some(1782943200000), Some(1782957600000)]),
        ("2026-07-01 10:00 pm", [Some(1782907200000), Some(1782943200000), Some(1782957600000)]),
        ("2026-07-01 12:00 AM", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("July 1, 2026 12:00 AM", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("July 1, 2026 13:00 PM", [None, None, None]),
        ("2026-07-01 PM", [None, None, None]),
        ("Tue Jul 01 2026", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("foo 2026-07-01", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("2026-07-01 foo", [None, None, None]),
        ("2026-07-01 (comment)", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("(x) 2026-07-01", [Some(1782828000000), Some(1782864000000), Some(1782878400000)]),
        ("2026-07-01 10:00:00 (EDT)", [Some(1782864000000), Some(1782900000000), Some(1782914400000)]),
        ("2026-07-01)", [None, None, None]),
        ("2026-07-01 10:00:00.5", [Some(1782864000500), Some(1782900000500), Some(1782914400500)]),
        ("2026-07-01 10:00:00.", [None, None, None]),
        ("2026-07-01 10:", [Some(1782864000000), Some(1782900000000), Some(1782914400000)]),
        ("2026-07-01 10::", [Some(1782864000000), Some(1782900000000), Some(1782914400000)]),
        ("2026-07-01 10:00:00:00", [Some(1782864000000), Some(1782900000000), Some(1782914400000)]),
        ("2026-07-01 10 20", [None, None, None]),
        ("10000-01-01", [Some(253402261200000), Some(253402300800000), Some(253402318800000)]),
        ("99-01-01", [Some(915109200000), Some(915148800000), Some(915166800000)]),
        ("50-01-01", [Some(-631188000000), Some(-631152000000), Some(-631134000000)]),
        ("49-01-01", [Some(2493032400000), Some(2493072000000), Some(2493090000000)]),
        ("1-1-1", [Some(978267600000), Some(978307200000), Some(978325200000)]),
        ("2026-07-01 10:00:00 GMT+300000:", [Some(702900000000), Some(702900000000), Some(702900000000)]),
        ("2026-07-01 10:00:00 GMT+12345", [None, None, None]),
        ("2026-07-01 10:00:00 GMT+", [None, None, None]),
        ("2026-07-01 10:00:00 GMT+1:30", [Some(1782894600000), Some(1782894600000), Some(1782894600000)]),
        ("Julyy 4 2026", [Some(1783087200000), Some(1783123200000), Some(1783137600000)]),
        ("Ju 4 2026", [None, None, None]),
        ("2026-07-01T10:00:00+05:30 extra", [None, None, None]),
        ("", [None, None, None]),
        (" ", [None, None, None]),
        ("abc", [None, None, None]),
        ("Infinity", [None, None, None]),
        ("1970-01-01T00:00:00Z", [Some(0), Some(0), Some(0)]),
        ("1969-12-31T23:59:59.999Z", [Some(-1), Some(-1), Some(-1)]),
        ("2026\u{0}2", [Some(1767225600000), Some(1767225600000), Some(1767225600000)]),
        ("2026-03-08T02:30:00", [Some(1772897400000), Some(1772937000000), Some(1772955000000)]),
        ("2026-11-01T01:30:00", [Some(1793457000000), Some(1793496600000), Some(1793511000000)]),
        ("2026-10-04T02:30:00", [Some(1791045000000), Some(1791081000000), Some(1791095400000)]),
        ("2026-04-05T02:30:00", [Some(1775316600000), Some(1775356200000), Some(1775370600000)]),
        ("2026-03-29T02:30:00", [Some(1774711800000), Some(1774751400000), Some(1774765800000)]),
        ("1850-06-01T00:00:00", [Some(-3773815492000), Some(-3773779200000), Some(-3773761438000)]),
        ("9999-12-31T23:00:00", [Some(253402257600000), Some(253402297200000), Some(253402315200000)]),
        ("+010000-01-01T00:00:00", [Some(253402261200000), Some(253402300800000), Some(253402318800000)]),
        ("+275760-09-13T00:00:00", [Some(8639999964000000), Some(8640000000000000), None]),
    ];

    #[test]
    fn date_parse_matches_node_in_three_time_zones() {
        let zones = ["Australia/Sydney", "UTC", "America/New_York"].map(|name| TimeZone::get(name).unwrap());
        let mut mismatches = Vec::new();
        for (input, expected) in NODE_DATES {
            for (zone, want) in zones.iter().zip(expected) {
                let got = parse_in(input, zone);
                if got != *want {
                    mismatches.push(format!("{input:?} in {}: got {got:?}, Node {want:?}", zone.iana_name().unwrap()));
                }
            }
        }
        assert!(mismatches.is_empty(), "{} mismatches:\n{}", mismatches.len(), mismatches.join("\n"));
    }
}
