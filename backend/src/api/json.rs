//! JSON exactly as v1 reads and writes it.
//!
//! Output: FastAPI's `JSONResponse` is `json.dumps(content, ensure_ascii=False, allow_nan=False,
//! separators=(",", ":"))`: keys in insertion order, floats as Python `repr` (`5.0`, `1e-05`), ints as
//! ints, non-ASCII unescaped, NaN/Infinity rejected (v1 answers 500). `J` keeps that distinction (an int
//! `100000` is not a float `100000.0`) and the key order.
//!
//! Input: request bodies are parsed like Python's `json.loads` (Starlette `request.json()`), including its
//! error messages and character positions, which FastAPI puts in its 422 `json_invalid` error; `NaN`,
//! `Infinity` and `-Infinity` are accepted; integers have arbitrary size; duplicate keys keep the first
//! position and the last value.

use crate::pyfmt;

#[derive(Debug, Clone, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Int(i64),
    /// an integer outside i64 (decimal digits, optional leading '-')
    Big(String),
    Float(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl From<i64> for J {
    fn from(v: i64) -> J {
        J::Int(v)
    }
}
impl From<f64> for J {
    fn from(v: f64) -> J {
        J::Float(v)
    }
}
impl From<bool> for J {
    fn from(v: bool) -> J {
        J::Bool(v)
    }
}
impl From<&str> for J {
    fn from(v: &str) -> J {
        J::Str(v.to_string())
    }
}
impl From<String> for J {
    fn from(v: String) -> J {
        J::Str(v)
    }
}
impl<T: Into<J>> From<Option<T>> for J {
    fn from(v: Option<T>) -> J {
        v.map_or(J::Null, Into::into)
    }
}
impl From<Vec<J>> for J {
    fn from(v: Vec<J>) -> J {
        J::Arr(v)
    }
}

/// An object literal with keys in the given order: `obj! { "a" => 1, "b" => x }`.
#[macro_export]
macro_rules! obj {
    ($($k:expr => $v:expr),* $(,)?) => {
        $crate::api::json::J::Obj(vec![$(($k.to_string(), $crate::api::json::J::from($v))),*])
    };
}

impl J {
    pub fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Append (or replace in place) a key of an object.
    pub fn set(&mut self, key: &str, v: impl Into<J>) {
        if let J::Obj(kv) = self {
            let v = v.into();
            match kv.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = v,
                None => kv.push((key.to_string(), v)),
            }
        }
    }

    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            J::Null => false,
            J::Bool(b) => *b,
            J::Int(i) => *i != 0,
            J::Big(_) => true,
            J::Float(f) => *f != 0.0,
            J::Str(s) => !s.is_empty(),
            J::Arr(a) => !a.is_empty(),
            J::Obj(o) => !o.is_empty(),
        }
    }

    /// `json.dumps(..., ensure_ascii=False, allow_nan=False, separators=(",", ":"))`; None if the value
    /// holds NaN or an infinity (Python raises ValueError).
    pub fn dumps(&self) -> Option<String> {
        let mut out = String::new();
        self.write(&mut out).then_some(out)
    }

    fn write(&self, out: &mut String) -> bool {
        match self {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::Int(i) => out.push_str(&i.to_string()),
            J::Big(s) => out.push_str(s),
            J::Float(f) => {
                if !f.is_finite() {
                    return false;
                }
                out.push_str(&pyfmt::repr(*f));
            }
            J::Str(s) => write_str(s, out),
            J::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    if !v.write(out) {
                        return false;
                    }
                }
                out.push(']');
            }
            J::Obj(kv) => {
                out.push('{');
                for (i, (k, v)) in kv.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(k, out);
                    out.push(':');
                    if !v.write(out) {
                        return false;
                    }
                }
                out.push('}');
            }
        }
        true
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------- parsing (Python json.loads)

/// A `json.JSONDecodeError`: message and character (not byte) position.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodeError {
    pub msg: String,
    pub pos: usize,
}

fn derr<T>(msg: impl Into<String>, pos: usize) -> Result<T, DecodeError> {
    Err(DecodeError { msg: msg.into(), pos })
}

/// Parse like `json.loads(text)` (text already decoded; Python strips a UTF-8 BOM when given bytes).
pub fn loads(text: &str) -> Result<J, DecodeError> {
    let s: Vec<char> = text.chars().collect();
    let p = Parser { s: &s };
    let start = p.ws(0);
    let (v, end) = match p.scan_once(start) {
        Ok(r) => r,
        Err(Stop(i)) => return derr("Expecting value", i),
        Err(Fail(e)) => return Err(e),
    };
    let end = p.ws(end);
    if end != s.len() {
        return derr("Extra data", end);
    }
    Ok(v)
}

enum ScanErr {
    /// `StopIteration(idx)`: no value starts here
    Stop(usize),
    Fail(DecodeError),
}
use ScanErr::{Fail, Stop};

impl From<DecodeError> for ScanErr {
    fn from(e: DecodeError) -> ScanErr {
        Fail(e)
    }
}

struct Parser<'a> {
    s: &'a [char],
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

impl Parser<'_> {
    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    /// `WHITESPACE.match(s, i).end()` (a position past the end is clamped, like `re`)
    fn ws(&self, mut i: usize) -> usize {
        i = i.min(self.s.len());
        while self.at(i).is_some_and(is_ws) {
            i += 1;
        }
        i
    }

    fn starts(&self, i: usize, lit: &str) -> bool {
        let n = lit.chars().count();
        i + n <= self.s.len() && self.s[i..i + n].iter().copied().eq(lit.chars())
    }

    fn scan_once(&self, idx: usize) -> Result<(J, usize), ScanErr> {
        let Some(c) = self.at(idx) else { return Err(Stop(idx)) };
        match c {
            '"' => {
                let (s, end) = self.scanstring(idx + 1)?;
                return Ok((J::Str(s), end));
            }
            '{' => return self.object(idx + 1),
            '[' => return self.array(idx + 1),
            'n' if self.starts(idx, "null") => return Ok((J::Null, idx + 4)),
            't' if self.starts(idx, "true") => return Ok((J::Bool(true), idx + 4)),
            'f' if self.starts(idx, "false") => return Ok((J::Bool(false), idx + 5)),
            _ => {}
        }
        if let Some(r) = self.number(idx) {
            return Ok(r);
        }
        if c == 'N' && self.starts(idx, "NaN") {
            return Ok((J::Float(f64::NAN), idx + 3));
        }
        if c == 'I' && self.starts(idx, "Infinity") {
            return Ok((J::Float(f64::INFINITY), idx + 8));
        }
        if c == '-' && self.starts(idx, "-Infinity") {
            return Ok((J::Float(f64::NEG_INFINITY), idx + 9));
        }
        Err(Stop(idx))
    }

    /// `(-?(?:0|[1-9][0-9]*))(\.[0-9]+)?([eE][-+]?[0-9]+)?`
    fn number(&self, idx: usize) -> Option<(J, usize)> {
        let digit = |i: usize| self.at(i).is_some_and(|c| c.is_ascii_digit());
        let mut i = idx;
        if self.at(i) == Some('-') {
            i += 1;
        }
        match self.at(i) {
            Some('0') => i += 1,
            Some('1'..='9') => {
                while digit(i) {
                    i += 1;
                }
            }
            _ => return None,
        }
        let int_end = i;
        let mut is_float = false;
        if self.at(i) == Some('.') && digit(i + 1) {
            i += 1;
            while digit(i) {
                i += 1;
            }
            is_float = true;
        }
        if matches!(self.at(i), Some('e' | 'E')) {
            let mut j = i + 1;
            if matches!(self.at(j), Some('+' | '-')) {
                j += 1;
            }
            if digit(j) {
                while digit(j) {
                    j += 1;
                }
                i = j;
                is_float = true;
            }
        }
        let text: String = self.s[idx..i].iter().collect();
        if is_float {
            // Python float(): correctly rounded; overflow gives inf
            let f = text.parse::<f64>().unwrap_or(f64::INFINITY);
            return Some((J::Float(f), i));
        }
        let _ = int_end;
        Some((int_from_digits(&text), i))
    }

    fn scanstring(&self, mut end: usize) -> Result<(String, usize), DecodeError> {
        let begin = end - 1;
        let mut out = String::new();
        loop {
            // chunk: everything up to '"', '\\' or a control character
            let mut i = end;
            loop {
                match self.at(i) {
                    None => return derr("Unterminated string starting at", begin),
                    Some(c) if c == '"' || c == '\\' || (c as u32) < 0x20 => break,
                    Some(c) => {
                        out.push(c);
                        i += 1;
                    }
                }
            }
            let term = self.at(i).unwrap();
            end = i + 1;
            if term == '"' {
                break;
            }
            if term != '\\' {
                return derr("Invalid control character at", i);
            }
            let Some(esc) = self.at(end) else { return derr("Unterminated string starting at", begin) };
            if esc != 'u' {
                let c = match esc {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    _ => return derr("Invalid \\escape", end - 1),
                };
                out.push(c);
                end += 1;
            } else {
                let mut uni = self.hex4(end)?;
                end += 5;
                if (0xd800..=0xdbff).contains(&uni) && self.at(end) == Some('\\') && self.at(end + 1) == Some('u') {
                    let uni2 = self.hex4(end + 1)?;
                    if (0xdc00..=0xdfff).contains(&uni2) {
                        uni = 0x10000 + (((uni - 0xd800) << 10) | (uni2 - 0xdc00));
                        end += 6;
                    }
                }
                // a lone surrogate cannot be held in a Rust string (Python keeps it): U+FFFD
                out.push(char::from_u32(uni).unwrap_or('\u{fffd}'));
            }
        }
        Ok((out, end))
    }

    /// `_decode_uXXXX(s, pos)`: the 4 hex digits after `s[pos] == 'u'`
    fn hex4(&self, pos: usize) -> Result<u32, DecodeError> {
        if pos + 5 <= self.s.len() {
            let h: String = self.s[pos + 1..pos + 5].iter().collect();
            if h.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(u32::from_str_radix(&h, 16).unwrap());
            }
        }
        derr("Invalid \\uXXXX escape", pos)
    }

    fn object(&self, mut end: usize) -> Result<(J, usize), ScanErr> {
        let mut pairs: Vec<(String, J)> = Vec::new();
        let mut nextchar = self.at(end);
        if nextchar != Some('"') {
            if nextchar.is_none_or(is_ws) {
                end = self.ws(end);
                nextchar = self.at(end);
            }
            if nextchar == Some('}') {
                return Ok((J::Obj(pairs), end + 1));
            } else if nextchar != Some('"') {
                return Err(Fail(DecodeError {
                    msg: "Expecting property name enclosed in double quotes".into(),
                    pos: end,
                }));
            }
        }
        end += 1;
        loop {
            let (key, e) = self.scanstring(end)?;
            end = e;
            if self.at(end) != Some(':') {
                end = self.ws(end);
                if self.at(end) != Some(':') {
                    return Err(Fail(DecodeError { msg: "Expecting ':' delimiter".into(), pos: end }));
                }
            }
            end += 1;
            if self.at(end).is_some_and(is_ws) {
                end += 1;
                if self.at(end).is_some_and(is_ws) {
                    end = self.ws(end + 1);
                }
            }
            let (value, e) = match self.scan_once(end) {
                Ok(r) => r,
                Err(Stop(i)) => return Err(Fail(DecodeError { msg: "Expecting value".into(), pos: i })),
                Err(f) => return Err(f),
            };
            end = e;
            match pairs.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value,
                None => pairs.push((key, value)),
            }
            let mut nc = self.at(end);
            if nc.is_some_and(is_ws) {
                end = self.ws(end + 1);
                nc = self.at(end);
            }
            end += 1;
            if nc == Some('}') {
                break;
            } else if nc != Some(',') {
                return Err(Fail(DecodeError { msg: "Expecting ',' delimiter".into(), pos: end - 1 }));
            }
            let comma = end - 1;
            end = self.ws(end);
            let nc = self.at(end);
            end += 1;
            if nc != Some('"') {
                if nc == Some('}') {
                    return Err(Fail(DecodeError {
                        msg: "Illegal trailing comma before end of object".into(),
                        pos: comma,
                    }));
                }
                return Err(Fail(DecodeError {
                    msg: "Expecting property name enclosed in double quotes".into(),
                    pos: end - 1,
                }));
            }
        }
        Ok((J::Obj(pairs), end))
    }

    fn array(&self, mut end: usize) -> Result<(J, usize), ScanErr> {
        let mut values = Vec::new();
        let mut nextchar = self.at(end);
        if nextchar.is_none_or(is_ws) {
            end = self.ws(end + 1);
            nextchar = self.at(end);
        }
        if nextchar == Some(']') {
            return Ok((J::Arr(values), end + 1));
        }
        loop {
            let (v, e) = match self.scan_once(end) {
                Ok(r) => r,
                Err(Stop(i)) => return Err(Fail(DecodeError { msg: "Expecting value".into(), pos: i })),
                Err(f) => return Err(f),
            };
            end = e;
            values.push(v);
            let mut nc = self.at(end);
            if nc.is_none_or(is_ws) {
                end = self.ws(end + 1);
                nc = self.at(end);
            }
            end += 1;
            if nc == Some(']') {
                break;
            } else if nc != Some(',') {
                return Err(Fail(DecodeError { msg: "Expecting ',' delimiter".into(), pos: end - 1 }));
            }
            let comma = end - 1;
            if self.at(end).is_some_and(is_ws) {
                end += 1;
                if self.at(end).is_some_and(is_ws) {
                    end = self.ws(end + 1);
                }
            }
            if self.at(end) == Some(']') {
                return Err(Fail(DecodeError { msg: "Illegal trailing comma before end of array".into(), pos: comma }));
            }
        }
        Ok((J::Arr(values), end))
    }
}

/// An integer literal as `J::Int`, or `J::Big` beyond i64.
pub fn int_from_digits(text: &str) -> J {
    match text.parse::<i64>() {
        Ok(i) => J::Int(i),
        Err(_) => {
            let (neg, digits) = match text.strip_prefix('-') {
                Some(d) => (true, d),
                None => (false, text.strip_prefix('+').unwrap_or(text)),
            };
            let digits = digits.trim_start_matches('0');
            let digits = if digits.is_empty() { "0" } else { digits };
            match digits.parse::<i64>() {
                Ok(i) => J::Int(if neg { -i } else { i }),
                Err(_) => J::Big(format!("{}{digits}", if neg { "-" } else { "" })),
            }
        }
    }
}

/// Python `repr()` of a one-character string.
pub fn py_repr_char(c: char) -> String {
    match c {
        '\n' => "'\\n'".into(),
        '\r' => "'\\r'".into(),
        '\t' => "'\\t'".into(),
        '\\' => "'\\\\'".into(),
        '\'' => "\"'\"".into(),
        c if (c as u32) < 0x20 || c as u32 == 0x7f => format!("'\\x{:02x}'", c as u32),
        c if (0x80..0xa0).contains(&(c as u32)) => format!("'\\x{:02x}'", c as u32),
        c => format!("'{c}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dumps_like_python() {
        let v = obj! {"a" => 1i64, "b" => 5.0, "c" => 1e-5, "d" => "芙莉莲\"\n\u{1}", "e" => J::Null, "f" => 1.5e16,
                      "g" => 0.0315, "h" => -1.3200000000069849e-05, "i" => 100000.0};
        assert_eq!(
            v.dumps().unwrap(),
            r#"{"a":1,"b":5.0,"c":1e-05,"d":"芙莉莲\"\n\u0001","e":null,"f":1.5e+16,"g":0.0315,"h":-1.3200000000069849e-05,"i":100000.0}"#
        );
        assert!(J::Arr(vec![J::Float(f64::NAN)]).dumps().is_none());
    }

    #[test]
    fn loads_like_python() {
        let v = loads(r#" {"a": [1, 2.50, -0, 1e400, NaN], "a": "x", "b": 123456789012345678901234567890} "#).unwrap();
        assert_eq!(v.get("a"), Some(&J::Str("x".into())));
        assert_eq!(v.get("b"), Some(&J::Big("123456789012345678901234567890".into())));
        let e = |t: &str| loads(t).unwrap_err();
        assert_eq!(e(r#"{"username": "alice","#), DecodeError { msg: "Expecting property name enclosed in double quotes".into(), pos: 21 });
        assert_eq!(e("{bad"), DecodeError { msg: "Expecting property name enclosed in double quotes".into(), pos: 1 });
        assert_eq!(e(""), DecodeError { msg: "Expecting value".into(), pos: 0 });
        assert_eq!(e("[1,]").msg, "Illegal trailing comma before end of array");
        assert_eq!(e("{\"a\":1,}").msg, "Illegal trailing comma before end of object");
        assert_eq!(e("{\"a\" 1}"), DecodeError { msg: "Expecting ':' delimiter".into(), pos: 5 });
        assert_eq!(e("1 2"), DecodeError { msg: "Extra data".into(), pos: 2 });
        assert_eq!(e("\"ab"), DecodeError { msg: "Unterminated string starting at".into(), pos: 0 });
        assert_eq!(e("\"a\nb\""), DecodeError { msg: "Invalid control character at".into(), pos: 2 });
        assert_eq!(e("\"\\x\""), DecodeError { msg: "Invalid \\escape".into(), pos: 1 });
        assert_eq!(e("\"\\u12\""), DecodeError { msg: "Invalid \\uXXXX escape".into(), pos: 2 });
        for (t, m, p) in [("[", "Expecting value", 1), ("{", "Expecting property name enclosed in double quotes", 1),
                          ("{\"a\":", "Expecting value", 5), ("[1 2]", "Expecting ',' delimiter", 3),
                          ("{\"a\":1 \"b\"}", "Expecting ',' delimiter", 7), ("-", "Expecting value", 0),
                          ("01", "Extra data", 1), ("1.", "Extra data", 1), ("[1,,2]", "Expecting value", 3),
                          ("{\"a\":1,,}", "Expecting property name enclosed in double quotes", 7),
                          ("  ", "Expecting value", 2), ("\"\\", "Unterminated string starting at", 0),
                          ("{\"a\"", "Expecting ':' delimiter", 4), ("[1,\n]", "Illegal trailing comma before end of array", 2),
                          ("{\"a\":1,\"b\"", "Expecting ':' delimiter", 10), ("{\"é\":\"\u{1}\"}", "Invalid control character at", 6)] {
            assert_eq!(e(t), DecodeError { msg: m.into(), pos: p }, "{t:?}");
        }
        assert_eq!(loads("\"\\ud83d\\ude00\"").unwrap(), J::Str("😀".into()));
    }
}
