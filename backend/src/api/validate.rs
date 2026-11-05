//! Request parsing and validation with FastAPI / pydantic v2 semantics, so that error responses are the
//! same JSON (`{"detail": [{"type", "loc", "msg", "input", "ctx"?}]}`) and the same inputs are accepted
//! (pydantic's lax mode: `"5"` and `5.0` are valid ints, `true` is 1, a JSON number is a valid price...).
//!
//! FastAPI's order, reproduced by the handlers: (1) read and decode the body (invalid JSON -> 422
//! `json_invalid`, undecodable -> 400), (2) dependencies (`current_player` -> 401), (3) path, query and
//! body validation errors together -> 422, (4) the endpoint.

use super::json::{int_from_digits, loads, J};
use crate::obj;

/// A validation error item (one entry of `detail`).
pub type Err = J;

pub fn error(kind: &str, loc: Vec<J>, msg: &str, input: J, ctx: Option<J>) -> Err {
    let mut e = obj! {"type" => kind, "loc" => J::Arr(loc), "msg" => msg, "input" => input};
    if let Some(c) = ctx {
        e.set("ctx", c);
    }
    e
}

fn loc(parts: &[&str]) -> Vec<J> {
    parts.iter().map(|p| J::from(*p)).collect()
}

// ---------------------------------------------------------------- body

/// What FastAPI hands to the body model after reading the request.
#[derive(Debug, Clone)]
pub enum Body {
    /// no body bytes
    Missing,
    Json(J),
    /// not JSON by content type: the raw bytes
    Raw(Vec<u8>),
}

/// A failure while reading the body, before any validation.
#[derive(Debug, Clone)]
pub enum BodyError {
    /// 422 json_invalid
    Json(Err),
    /// 400 "There was an error parsing the body"
    Undecodable,
}

/// `email.message.Message().get_content_type()` style check for `application/json` or `*+json`.
fn is_json_content_type(v: &str) -> bool {
    let ctype = v.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if ctype.matches('/').count() != 1 {
        return false;
    }
    let (main, sub) = ctype.split_once('/').unwrap();
    main == "application" && (sub == "json" || sub.ends_with("+json"))
}

/// Read a request body like FastAPI 0.162 with `strict_content_type=True`.
pub fn read_body(bytes: &[u8], content_type: Option<&str>) -> Result<Body, BodyError> {
    if bytes.is_empty() {
        return Ok(Body::Missing);
    }
    let json = matches!(content_type, Some(ct) if !ct.is_empty() && is_json_content_type(ct));
    if !json {
        return Ok(Body::Raw(bytes.to_vec()));
    }
    // json.loads(bytes): UTF-8 with an optional BOM (UTF-16/32 detection is not reproduced)
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| BodyError::Undecodable)?;
    match loads(text) {
        Ok(v) => Ok(Body::Json(v)),
        Err(e) => Err(BodyError::Json(error(
            "json_invalid",
            vec![J::from("body"), J::Int(e.pos as i64)],
            "JSON decode error",
            obj! {},
            Some(obj! {"error" => e.msg}),
        ))),
    }
}

/// The fields of a JSON object body, or the errors for a body that is not an object.
pub fn body_fields(body: &Body) -> Result<&Vec<(String, J)>, Vec<Err>> {
    match body {
        Body::Json(J::Obj(kv)) => Ok(kv),
        Body::Missing => Err(vec![error("missing", loc(&["body"]), "Field required", J::Null, None)]),
        Body::Json(other) => Err(vec![model_type(other.clone())]),
        // jsonable_encoder(bytes) decodes them as UTF-8
        Body::Raw(b) => Err(vec![model_type(J::Str(String::from_utf8_lossy(b).into_owned()))]),
    }
}

fn model_type(input: J) -> Err {
    error(
        "model_attributes_type",
        loc(&["body"]),
        "Input should be a valid dictionary or object to extract fields from",
        input,
        None,
    )
}

fn field<'a>(kv: &'a [(String, J)], name: &str) -> Option<&'a J> {
    kv.iter().find(|(k, _)| k == name).map(|(_, v)| v)
}

fn missing(kv: &[(String, J)], name: &str) -> Err {
    error("missing", loc(&["body", name]), "Field required", J::Obj(kv.to_vec()), None)
}

// ---------------------------------------------------------------- scalar validators

/// `str` with optional length bounds (characters = code points).
fn v_str(v: &J, l: &[&str], min: Option<usize>, max: Option<usize>) -> Result<String, Err> {
    let J::Str(s) = v else {
        return Err(error("string_type", loc(l), "Input should be a valid string", v.clone(), None));
    };
    let n = s.chars().count();
    if let Some(m) = min {
        if n < m {
            let msg = format!("String should have at least {m} character{}", if m == 1 { "" } else { "s" });
            return Err(error("string_too_short", loc(l), &msg, v.clone(), Some(obj! {"min_length" => m as i64})));
        }
    }
    if let Some(m) = max {
        if n > m {
            let msg = format!("String should have at most {m} character{}", if m == 1 { "" } else { "s" });
            return Err(error("string_too_long", loc(l), &msg, v.clone(), Some(obj! {"max_length" => m as i64})));
        }
    }
    Ok(s.clone())
}

/// An integer value of any size: i64, or out of range with its sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BigInt {
    Small(i64),
    /// out of i64 range, positive (true) or negative (false)
    Huge(bool),
}

impl BigInt {
    fn cmp_i64(self, k: i64) -> std::cmp::Ordering {
        match self {
            BigInt::Small(v) => v.cmp(&k),
            BigInt::Huge(true) => std::cmp::Ordering::Greater,
            BigInt::Huge(false) => std::cmp::Ordering::Less,
        }
    }
}

const INT_PARSING: &str = "Input should be a valid integer, unable to parse string as an integer";

/// pydantic's lax int from a string: surrounding whitespace, a sign, digits with single underscores
/// between them, and an optional fraction of zeros (`"5.00"`).
pub fn parse_int_str(s: &str) -> Option<BigInt> {
    let t = s.trim();
    let (neg, body) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    if let Some(f) = frac {
        if f.is_empty() || !f.bytes().all(|b| b == b'0') {
            return None;
        }
    }
    if int.is_empty() || int.starts_with('_') || int.ends_with('_') || int.contains("__") {
        return None;
    }
    if !int.bytes().all(|b| b.is_ascii_digit() || b == b'_') {
        return None;
    }
    let digits: String = int.chars().filter(|c| *c != '_').collect();
    Some(match int_from_digits(&format!("{}{digits}", if neg { "-" } else { "" })) {
        J::Int(i) => BigInt::Small(i),
        _ => BigInt::Huge(!neg),
    })
}

/// pydantic lax `int` from a JSON value.
fn v_int(v: &J, l: &[&str]) -> Result<BigInt, Err> {
    let e = |kind: &str, msg: &str| error(kind, loc(l), msg, v.clone(), None);
    match v {
        J::Int(i) => Ok(BigInt::Small(*i)),
        J::Big(s) => Ok(BigInt::Huge(!s.starts_with('-'))),
        J::Bool(b) => Ok(BigInt::Small(*b as i64)),
        J::Float(f) => {
            if !f.is_finite() {
                Err(e("finite_number", "Input should be a finite number"))
            } else if f.fract() != 0.0 {
                Err(e("int_from_float", "Input should be a valid integer, got a number with a fractional part"))
            } else if f.abs() >= i64::MAX as f64 {
                Err(e("int_parsing_size", "Unable to parse input string as an integer, exceeded maximum size"))
            } else {
                Ok(BigInt::Small(*f as i64))
            }
        }
        J::Str(s) => parse_int_str(s).ok_or_else(|| e("int_parsing", INT_PARSING)),
        _ => Err(e("int_type", "Input should be a valid integer")),
    }
}

#[derive(Clone, Copy)]
enum Bound {
    Gt(i64),
    Ge(i64),
    Le(i64),
}

fn check_bounds(n: BigInt, v: &J, l: &[&str], bounds: &[Bound]) -> Result<BigInt, Err> {
    use std::cmp::Ordering::*;
    for b in bounds {
        let (ok, kind, msg, ctx) = match *b {
            Bound::Gt(k) => (n.cmp_i64(k) == Greater, "greater_than", format!("Input should be greater than {k}"), obj! {"gt" => k}),
            Bound::Ge(k) => (
                n.cmp_i64(k) != Less,
                "greater_than_equal",
                format!("Input should be greater than or equal to {k}"),
                obj! {"ge" => k},
            ),
            Bound::Le(k) => (
                n.cmp_i64(k) != Greater,
                "less_than_equal",
                format!("Input should be less than or equal to {k}"),
                obj! {"le" => k},
            ),
        };
        if !ok {
            return Err(error(kind, loc(l), &msg, v.clone(), Some(ctx)));
        }
    }
    Ok(n)
}

/// A path or query parameter declared `int`.
pub fn param_int(kind: &str, name: &str, raw: &str) -> Result<BigInt, Err> {
    parse_int_str(raw).ok_or_else(|| error("int_parsing", loc(&[kind, name]), INT_PARSING, J::from(raw), None))
}

// ---------------------------------------------------------------- models

/// `Credentials`: username 2..=20, password 6..=100, invite `str | None = None`.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    pub invite: Option<String>,
}

pub fn credentials(body: &Body) -> Result<Credentials, Vec<Err>> {
    let kv = body_fields(body)?;
    let mut errs = Vec::new();
    let username = match field(kv, "username") {
        None => {
            errs.push(missing(kv, "username"));
            None
        }
        Some(v) => v_str(v, &["body", "username"], Some(2), Some(20)).map_err(|e| errs.push(e)).ok(),
    };
    let password = match field(kv, "password") {
        None => {
            errs.push(missing(kv, "password"));
            None
        }
        Some(v) => v_str(v, &["body", "password"], Some(6), Some(100)).map_err(|e| errs.push(e)).ok(),
    };
    let invite = match field(kv, "invite") {
        None | Some(J::Null) => Some(None),
        Some(v) => v_str(v, &["body", "invite"], None, None).map(Some).map_err(|e| errs.push(e)).ok(),
    };
    match (username, password, invite) {
        (Some(username), Some(password), Some(invite)) if errs.is_empty() => Ok(Credentials { username, password, invite }),
        _ => Err(errs),
    }
}

/// `OrderIn.price: float | str | None`
#[derive(Debug, Clone, PartialEq)]
pub enum PriceIn {
    None,
    Float(f64),
    Str(String),
}

/// `OrderIn`: company_id int, side str, kind str = "limit", qty int (0 < qty <= 10,000,000),
/// price float | str | None = None.
#[derive(Debug, Clone)]
pub struct OrderIn {
    pub company_id: BigInt,
    pub side: String,
    pub kind: String,
    pub qty: i64,
    pub price: PriceIn,
}

pub fn order_in(body: &Body) -> Result<OrderIn, Vec<Err>> {
    let kv = body_fields(body)?;
    let mut errs = Vec::new();
    let company_id = match field(kv, "company_id") {
        None => {
            errs.push(missing(kv, "company_id"));
            None
        }
        Some(v) => v_int(v, &["body", "company_id"]).map_err(|e| errs.push(e)).ok(),
    };
    let side = match field(kv, "side") {
        None => {
            errs.push(missing(kv, "side"));
            None
        }
        Some(v) => v_str(v, &["body", "side"], None, None).map_err(|e| errs.push(e)).ok(),
    };
    let kind = match field(kv, "kind") {
        None => Some("limit".to_string()),
        Some(v) => v_str(v, &["body", "kind"], None, None).map_err(|e| errs.push(e)).ok(),
    };
    let qty = match field(kv, "qty") {
        None => {
            errs.push(missing(kv, "qty"));
            None
        }
        Some(v) => v_int(v, &["body", "qty"])
            .and_then(|n| check_bounds(n, v, &["body", "qty"], &[Bound::Gt(0), Bound::Le(10_000_000)]))
            .map_err(|e| errs.push(e))
            .ok(),
    };
    let price = match field(kv, "price") {
        None | Some(J::Null) => Some(PriceIn::None),
        Some(J::Str(s)) => Some(PriceIn::Str(s.clone())),
        Some(J::Float(f)) => Some(PriceIn::Float(*f)),
        Some(J::Int(i)) => Some(PriceIn::Float(*i as f64)),
        Some(J::Bool(b)) => Some(PriceIn::Float(*b as i64 as f64)),
        Some(v @ J::Big(s)) => match s.parse::<f64>() {
            Ok(f) if f.is_finite() => Some(PriceIn::Float(f)),
            _ => {
                price_errors(v, &mut errs);
                None
            }
        },
        Some(v) => {
            price_errors(v, &mut errs);
            None
        }
    };
    match (company_id, side, kind, qty, price) {
        (Some(company_id), Some(side), Some(kind), Some(BigInt::Small(qty)), Some(price)) if errs.is_empty() => {
            Ok(OrderIn { company_id, side, kind, qty, price })
        }
        _ => Err(errs),
    }
}

fn price_errors(v: &J, errs: &mut Vec<Err>) {
    errs.push(error("float_type", loc(&["body", "price", "float"]), "Input should be a valid number", v.clone(), None));
    errs.push(error("string_type", loc(&["body", "price", "str"]), "Input should be a valid string", v.clone(), None));
}

/// `SubscribeIn`: qty int, 0 <= qty <= 10,000,000.
pub fn subscribe_in(body: &Body) -> Result<i64, Vec<Err>> {
    let kv = body_fields(body)?;
    match field(kv, "qty") {
        None => Err(vec![missing(kv, "qty")]),
        Some(v) => match v_int(v, &["body", "qty"])
            .and_then(|n| check_bounds(n, v, &["body", "qty"], &[Bound::Ge(0), Bound::Le(10_000_000)]))
        {
            Ok(BigInt::Small(q)) => Ok(q),
            Ok(_) => unreachable!("bounded"),
            Err(e) => Err(vec![e]),
        },
    }
}

// ---------------------------------------------------------------- prices (v1 to_cents)

/// Outcome of v1 `to_cents(value)` on `Decimal(str(value))`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cents {
    Ok(i64),
    /// 400 价格格式不对 (InvalidOperation from the constructor)
    BadFormat,
    /// 400 价格最多两位小数，且必须大于 0
    BadValue,
    /// v1 raises an uncaught decimal.InvalidOperation: 500 (NaN comparison, quantize overflow)
    Crash,
}

/// v1 `to_cents`. Floats go through Python `str(float)`; `None` is `Decimal("None")`.
pub fn to_cents(p: &PriceIn) -> Cents {
    match p {
        PriceIn::None => Cents::BadFormat,
        PriceIn::Float(f) => decimal_cents(&crate::pyfmt::repr(*f)),
        PriceIn::Str(s) => decimal_cents(s),
    }
}

/// Unicode decimal digit value (Python's Decimal accepts any Nd digit; the common blocks are covered).
fn digit_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return c.to_digit(10);
    }
    const ZEROS: [u32; 12] =
        [0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0E50, 0x1040, 0xFF10];
    let u = c as u32;
    ZEROS.iter().find(|z| (**z..**z + 10).contains(&u)).map(|z| u - z)
}

/// `Decimal(s)` then the checks in `to_cents`.
pub fn decimal_cents(s: &str) -> Cents {
    let t = s.trim();
    let (neg, rest) = match t.chars().next() {
        Some('-') => (true, &t[1..]),
        Some('+') => (false, &t[1..]),
        _ => (false, t),
    };
    let lower = rest.to_lowercase();
    if lower == "inf" || lower == "infinity" {
        return if neg { Cents::BadValue } else { Cents::Crash };
    }
    let nan_payload = |p: &str| p.chars().all(|c| digit_value(c).is_some());
    if let Some(p) = lower.strip_prefix("snan").or_else(|| lower.strip_prefix("nan")) {
        return if nan_payload(p) { Cents::Crash } else { Cents::BadFormat };
    }
    // digits [. digits] [e [sign] digits], at least one digit; '_' only between digits
    let chars: Vec<char> = rest.chars().collect();
    let mut i = 0;
    let mut coef = String::new();
    let mut frac_len: i64 = 0;
    let mut seen_digit = false;
    let mut in_frac = false;
    while i < chars.len() {
        let c = chars[i];
        if let Some(d) = digit_value(c) {
            coef.push(char::from_digit(d, 10).unwrap());
            seen_digit = true;
            if in_frac {
                frac_len += 1;
            }
        } else if c == '_' {
            let prev = i > 0 && digit_value(chars[i - 1]).is_some();
            let next = chars.get(i + 1).is_some_and(|n| digit_value(*n).is_some());
            if !(prev && next) {
                return Cents::BadFormat;
            }
        } else if c == '.' && !in_frac {
            in_frac = true;
        } else {
            break;
        }
        i += 1;
    }
    if !seen_digit {
        return Cents::BadFormat;
    }
    let mut exp: i64 = 0;
    if i < chars.len() {
        if chars[i] != 'e' && chars[i] != 'E' {
            return Cents::BadFormat;
        }
        i += 1;
        let eneg = match chars.get(i) {
            Some('-') => {
                i += 1;
                true
            }
            Some('+') => {
                i += 1;
                false
            }
            _ => false,
        };
        let mut any = false;
        while i < chars.len() {
            let c = chars[i];
            if let Some(d) = digit_value(c) {
                exp = exp.saturating_mul(10).saturating_add(d as i64);
                any = true;
            } else if c == '_' {
                let ok = digit_value(chars[i - 1]).is_some() && chars.get(i + 1).is_some_and(|n| digit_value(*n).is_some());
                if !ok {
                    return Cents::BadFormat;
                }
            } else {
                return Cents::BadFormat;
            }
            i += 1;
        }
        if !any {
            return Cents::BadFormat;
        }
        if eneg {
            exp = -exp;
        }
    }
    let exp = exp.saturating_sub(frac_len);
    let coef = coef.trim_start_matches('0');
    if coef.is_empty() || neg {
        return Cents::BadValue; // d <= 0
    }
    // d * 100 = coef * 10^k; quantize(0.01) rounds it half-even to an integer R, which must fit 28 digits
    let k = exp.saturating_add(2);
    const PREC: i64 = 28;
    if k >= 0 {
        if coef.len() as i64 + k > PREC {
            return Cents::Crash;
        }
        let r: i128 = coef.parse::<i128>().unwrap() * 10i128.pow(k as u32);
        return Cents::Ok(i64::try_from(r).unwrap_or(i64::MAX));
    }
    let drop = (-k) as usize;
    let (head, tail) = if coef.len() > drop { coef.split_at(coef.len() - drop) } else { ("", coef) };
    let tail_full = format!("{}{tail}", "0".repeat(drop - tail.len()));
    let mut r: i128 = if head.is_empty() {
        0
    } else if head.len() as i64 > PREC + 1 {
        return Cents::Crash;
    } else {
        head.parse().unwrap()
    };
    let first = tail_full.as_bytes()[0];
    let rest_zero = tail_full[1..].bytes().all(|b| b == b'0');
    let exact = first == b'0' && rest_zero;
    if first > b'5' || (first == b'5' && (!rest_zero || r % 2 == 1)) {
        r += 1;
    }
    if r.to_string().len() as i64 > PREC {
        return Cents::Crash;
    }
    if !exact {
        return Cents::BadValue;
    }
    Cents::Ok(i64::try_from(r).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cents_like_v1() {
        use Cents::*;
        for (s, want) in [
            ("5", Ok(500)),
            ("5.0", Ok(500)),
            ("5.10", Ok(510)),
            (" 5 ", Ok(500)),
            ("5_0", Ok(5000)),
            ("1e2", Ok(10000)),
            ("1E-2", Ok(1)),
            ("+4.99", Ok(499)),
            ("5.005", BadValue),
            ("-1", BadValue),
            ("0", BadValue),
            ("-0", BadValue),
            ("0.001", BadValue),
            ("abc", BadFormat),
            ("", BadFormat),
            ("5.", Ok(500)),
            (".5", Ok(50)),
            ("_5", BadFormat),
            ("5__0", BadFormat),
            ("NaN", Crash),
            ("-nan", Crash),
            ("Infinity", Crash),
            ("-inf", BadValue),
            ("1e+30", Crash),
            ("1e400", Crash),
            ("1e-07", BadValue),
            ("12345678901234567890123456.78", Ok(i64::MAX)),
            ("123456789012345678901234567.8", Crash),
            ("５", Ok(500)),
        ] {
            assert_eq!(decimal_cents(s), want, "{s:?}");
        }
        assert_eq!(to_cents(&PriceIn::Float(4.99)), Ok(499));
        assert_eq!(to_cents(&PriceIn::Float(0.30000000000000004)), BadValue);
        assert_eq!(to_cents(&PriceIn::None), BadFormat);
    }

    #[test]
    fn ints_like_pydantic() {
        for (s, ok) in [("5", true), (" 5 ", true), ("5.0", true), ("5.00", true), ("5.5", false), ("1_000", true),
                        ("+5", true), ("0x10", false), ("", false), ("1e2", false), ("5.", false), (".5", false),
                        ("-0", true), ("1__0", false)] {
            assert_eq!(parse_int_str(s).is_some(), ok, "{s:?}");
        }
        assert_eq!(parse_int_str("111111111111111111111111111111"), Some(BigInt::Huge(true)));
    }
}
