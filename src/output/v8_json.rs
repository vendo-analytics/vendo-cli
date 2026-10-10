//! V8's `JSON.parse` error messages (VE-3728), for what the TS CLI printed when a 2xx response
//! body or a `--*-file` wasn't JSON: a port of the error paths in V8's `JsonParser`
//! (src/json/json-parser.cc). serde_json does the real parsing; this only words the error.

/// V8's message for `JSON.parse(text)`, or `None` when V8 accepts the text.
pub fn parse_error(text: &str) -> Option<String> {
    let source: Vec<u16> = text.encode_utf16().collect();
    let mut parser = Parser { source: &source, pos: 0 };
    parser.parse().err().map(|error| parser.message(error))
}

/// V8's `one_char_json_tokens`.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Token {
    String,
    Number,
    True,
    False,
    Null,
    Whitespace,
    Colon,
    Comma,
    LBrack,
    RBrack,
    LBrace,
    RBrace,
    Illegal,
    Eos,
}

/// The message templates the parser reports with a position.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Template {
    UnterminatedString,
    ExpectedPropNameOrRBrace,
    ExpectedCommaOrRBrack,
    ExpectedCommaOrRBrace,
    NonWhitespaceAfterJson,
    BadEscapedCharacter,
    BadControlCharacter,
    BadUnicodeEscape,
    NoNumberAfterMinusSign,
    ExponentPartMissingNumber,
    UnterminatedFractionalNumber,
    ExpectedColonAfterPropertyName,
    ExpectedDoubleQuotedPropertyName,
}

/// What `ReportUnexpectedToken` was called with; the position is the parser's.
enum Error {
    /// No template: the message depends on the token (`LookUpErrorMessageForJsonToken`).
    Token(Token),
    Template(Template),
}

#[derive(PartialEq)]
enum Container {
    Object,
    Array,
}

struct Parser<'a> {
    source: &'a [u16],
    pos: usize,
}

const MAX_CONTEXT_CHARACTERS: usize = 10;
const MIN_LENGTH_FOR_CONTEXT: usize = MAX_CONTEXT_CHARACTERS * 2 + 1;

fn token_of(unit: Option<u16>) -> Token {
    let Some(unit) = unit else { return Token::Eos };
    match unit {
        0x22 => Token::String,
        0x2D | 0x30..=0x39 => Token::Number,
        0x74 => Token::True,
        0x66 => Token::False,
        0x6E => Token::Null,
        0x20 | 0x09 | 0x0A | 0x0D => Token::Whitespace,
        0x3A => Token::Colon,
        0x2C => Token::Comma,
        0x5B => Token::LBrack,
        0x5D => Token::RBrack,
        0x7B => Token::LBrace,
        0x7D => Token::RBrace,
        _ => Token::Illegal,
    }
}

fn is_digit(unit: Option<u16>) -> bool {
    matches!(unit, Some(0x30..=0x39))
}

impl Parser<'_> {
    fn current(&self) -> Option<u16> {
        self.source.get(self.pos).copied()
    }

    fn peek(&self) -> Token {
        token_of(self.current())
    }

    fn skip_whitespace(&mut self) {
        while self.peek() == Token::Whitespace {
            self.pos += 1;
        }
    }

    /// `ParseJsonValue` and `ParseJson`, iterative like V8's.
    fn parse(&mut self) -> Result<(), Error> {
        let mut stack: Vec<Container> = Vec::new();
        'value: loop {
            self.skip_whitespace();
            match self.peek() {
                Token::LBrace => {
                    self.pos += 1;
                    self.skip_whitespace();
                    if self.peek() == Token::RBrace {
                        self.pos += 1;
                    } else {
                        self.property_key(Template::ExpectedPropNameOrRBrace)?;
                        stack.push(Container::Object);
                        continue 'value;
                    }
                }
                Token::LBrack => {
                    self.pos += 1;
                    self.skip_whitespace();
                    if self.peek() == Token::RBrack {
                        self.pos += 1;
                    } else {
                        stack.push(Container::Array);
                        continue 'value;
                    }
                }
                Token::String => {
                    self.pos += 1;
                    self.scan_string()?;
                }
                Token::Number => self.scan_number()?,
                Token::True => self.scan_literal("true")?,
                Token::False => self.scan_literal("false")?,
                Token::Null => self.scan_literal("null")?,
                token => return Err(Error::Token(token)),
            }
            // A value is done: close the containers it finishes.
            loop {
                self.skip_whitespace();
                match stack.last() {
                    None if self.peek() == Token::Eos => return Ok(()),
                    None => return Err(Error::Template(Template::NonWhitespaceAfterJson)),
                    Some(Container::Object) => match self.peek() {
                        Token::Comma => {
                            self.pos += 1;
                            self.skip_whitespace();
                            self.property_key(Template::ExpectedDoubleQuotedPropertyName)?;
                            continue 'value;
                        }
                        Token::RBrace => {
                            self.pos += 1;
                            stack.pop();
                        }
                        _ => return Err(Error::Template(Template::ExpectedCommaOrRBrace)),
                    },
                    Some(Container::Array) => match self.peek() {
                        Token::Comma => {
                            self.pos += 1;
                            continue 'value;
                        }
                        Token::RBrack => {
                            self.pos += 1;
                            stack.pop();
                        }
                        _ => return Err(Error::Template(Template::ExpectedCommaOrRBrack)),
                    },
                }
            }
        }
    }

    /// `ExpectNext(STRING, missing)`, the key, then `ExpectNext(COLON, …)`.
    fn property_key(&mut self, missing: Template) -> Result<(), Error> {
        if self.peek() != Token::String {
            return Err(Error::Template(missing));
        }
        self.pos += 1;
        self.scan_string()?;
        self.skip_whitespace();
        if self.peek() != Token::Colon {
            return Err(Error::Template(Template::ExpectedColonAfterPropertyName));
        }
        self.pos += 1;
        Ok(())
    }

    /// `ScanJsonString`, after the opening quote.
    fn scan_string(&mut self) -> Result<(), Error> {
        loop {
            let Some(unit) = self.current() else { return Err(Error::Template(Template::UnterminatedString)) };
            match unit {
                0x22 => {
                    self.pos += 1;
                    return Ok(());
                }
                0x5C => {
                    self.pos += 1;
                    match self.current() {
                        None => return Err(Error::Token(Token::Eos)),
                        Some(c) if c > 0xFF => return Err(Error::Token(Token::Illegal)),
                        Some(0x22 | 0x5C | 0x2F | 0x62 | 0x66 | 0x6E | 0x72 | 0x74) => self.pos += 1,
                        Some(0x75) => {
                            for _ in 0..4 {
                                self.pos += 1;
                                if !self
                                    .current()
                                    .is_some_and(|c| char::from_u32(c.into()).is_some_and(|c| c.is_ascii_hexdigit()))
                                {
                                    return Err(Error::Template(Template::BadUnicodeEscape));
                                }
                            }
                            self.pos += 1;
                        }
                        Some(_) => return Err(Error::Template(Template::BadEscapedCharacter)),
                    }
                }
                0..0x20 => return Err(Error::Template(Template::BadControlCharacter)),
                _ => self.pos += 1,
            }
        }
    }

    /// `ParseJsonNumber`'s checks: a minus sign needs a digit, a leading zero can't be followed
    /// by one, a fraction and an exponent need at least one digit each.
    fn scan_number(&mut self) -> Result<(), Error> {
        if self.current() == Some(0x2D) {
            self.pos += 1;
        }
        if self.current() == Some(0x30) {
            self.pos += 1;
            if is_digit(self.current()) {
                return Err(Error::Token(Token::Number));
            }
        } else if is_digit(self.current()) {
            self.skip_digits();
        } else {
            return Err(Error::Template(Template::NoNumberAfterMinusSign));
        }
        if self.current() == Some(0x2E) {
            self.pos += 1;
            if !is_digit(self.current()) {
                return Err(Error::Template(Template::UnterminatedFractionalNumber));
            }
            self.skip_digits();
        }
        if matches!(self.current(), Some(0x65 | 0x45)) {
            self.pos += 1;
            if matches!(self.current(), Some(0x2B | 0x2D)) {
                self.pos += 1;
            }
            if !is_digit(self.current()) {
                return Err(Error::Template(Template::ExponentPartMissingNumber));
            }
            self.skip_digits();
        }
        Ok(())
    }

    fn skip_digits(&mut self) {
        while is_digit(self.current()) {
            self.pos += 1;
        }
    }

    /// `ScanLiteral`: the first character already matched.
    fn scan_literal(&mut self, literal: &str) -> Result<(), Error> {
        let expected: Vec<u16> = literal.encode_utf16().collect();
        let remaining = self.source.len() - self.pos;
        if remaining >= expected.len() && self.source[self.pos + 1..self.pos + expected.len()] == expected[1..] {
            self.pos += expected.len();
            return Ok(());
        }
        self.pos += 1;
        for &unit in expected[1..].iter().take(remaining - 1) {
            if self.current() != Some(unit) {
                return Err(Error::Token(token_of(self.current())));
            }
            self.pos += 1;
        }
        Err(Error::Token(Token::Eos))
    }

    /// `ReportUnexpectedToken`'s message for an error at the current position.
    fn message(&self, error: Error) -> String {
        let template = match error {
            Error::Template(template) => template,
            Error::Token(Token::Eos) => return "Unexpected end of JSON input".to_string(),
            Error::Token(Token::Number) => return self.at("Unexpected number in JSON"),
            Error::Token(Token::String) => return self.at("Unexpected string in JSON"),
            Error::Token(_) => return self.unexpected_token(),
        };
        self.at(match template {
            Template::UnterminatedString => "Unterminated string in JSON",
            Template::ExpectedPropNameOrRBrace => "Expected property name or '}' in JSON",
            Template::ExpectedCommaOrRBrack => "Expected ',' or ']' after array element in JSON",
            Template::ExpectedCommaOrRBrace => "Expected ',' or '}' after property value in JSON",
            Template::NonWhitespaceAfterJson => "Unexpected non-whitespace character after JSON",
            Template::BadEscapedCharacter => "Bad escaped character in JSON",
            Template::BadControlCharacter => "Bad control character in string literal in JSON",
            Template::BadUnicodeEscape => "Bad Unicode escape in JSON",
            Template::NoNumberAfterMinusSign => "No number after minus sign in JSON",
            Template::ExponentPartMissingNumber => "Exponent part is missing a number in JSON",
            Template::UnterminatedFractionalNumber => "Unterminated fractional number in JSON",
            Template::ExpectedColonAfterPropertyName => "Expected ':' after property name in JSON",
            Template::ExpectedDoubleQuotedPropertyName => "Expected double-quoted property name in JSON",
        })
    }

    /// `<what> at position N (line L column C)`; `\r\n` counts as one line break.
    fn at(&self, what: &str) -> String {
        let (mut line, mut line_start, mut i) = (1, 0, 0);
        while i < self.pos {
            if self.source[i] == 0x0D && i + 1 < self.pos && self.source[i + 1] == 0x0A {
                i += 1;
            }
            if matches!(self.source[i], 0x0D | 0x0A) {
                line += 1;
                line_start = i + 1;
            }
            i += 1;
        }
        format!("{what} at position {} (line {line} column {})", self.pos, 1 + i - line_start)
    }

    /// The token message with up to 10 characters of context either side.
    fn unexpected_token(&self) -> String {
        let text = |range: std::ops::Range<usize>| String::from_utf16_lossy(&self.source[range]);
        let source = text(0..self.source.len());
        if matches!(source.as_str(), "NaN" | "Infinity" | "undefined" | "[object Object]") {
            return format!("\"{source}\" is not valid JSON");
        }
        let token = text(self.pos..self.pos + 1);
        let (pos, length) = (self.pos, self.source.len());
        if length < MIN_LENGTH_FOR_CONTEXT {
            format!("Unexpected token '{token}', \"{source}\" is not valid JSON")
        } else if pos < MAX_CONTEXT_CHARACTERS {
            format!("Unexpected token '{token}', \"{}\"... is not valid JSON", text(0..pos + MAX_CONTEXT_CHARACTERS))
        } else if pos < length - MAX_CONTEXT_CHARACTERS {
            let context = text(pos - MAX_CONTEXT_CHARACTERS..pos + MAX_CONTEXT_CHARACTERS);
            format!("Unexpected token '{token}', ...\"{context}\"... is not valid JSON")
        } else {
            format!(
                "Unexpected token '{token}', ...\"{}\" is not valid JSON",
                text(pos - MAX_CONTEXT_CHARACTERS..length)
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Every case in the fixture, generated from Node by `scripts/gen-json-error-fixture.mjs`.
    #[test]
    fn errors_match_v8_word_for_word() {
        let fixture: Value = serde_json::from_str(include_str!("../../tests/fixtures/node-json-errors.json")).unwrap();
        let mut mismatches = Vec::new();
        let cases = fixture["cases"].as_array().unwrap();
        for case in cases {
            let text = case[0].as_str().unwrap();
            let want = case[1].as_str();
            let got = parse_error(text);
            if got.as_deref() != want {
                mismatches.push(format!("{text:?}: got {got:?}, V8 {want:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{} of {} differ:\n{}", mismatches.len(), cases.len(), mismatches.join("\n"));
    }
}
