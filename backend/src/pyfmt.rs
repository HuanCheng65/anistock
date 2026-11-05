//! Python number formatting used in v1's news texts and state rows, reproduced exactly.
//! Rust's fixed-precision float formatting is correctly rounded with ties to even, like CPython's.

/// `f"{x:.{prec}f}"`
pub fn fixed(x: f64, prec: usize) -> String {
    format!("{x:.prec$}")
}

/// Insert `,` thousands separators into the integer part of a formatted number.
fn group(s: &str) -> String {
    let (sign, rest) = match s.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", s),
    };
    let (int, frac) = match rest.find('.') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let mut out = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{sign}{out}{frac}")
}

/// `f"{x:,.{prec}f}"`
pub fn fixed_grouped(x: f64, prec: usize) -> String {
    group(&fixed(x, prec))
}

/// `f"{n:,}"` for an int
pub fn int_grouped(n: i64) -> String {
    group(&n.to_string())
}

/// `f"{x:.{prec}%}"`: multiply by 100 in floating point, fixed format, append `%`.
pub fn pct(x: f64, prec: usize) -> String {
    format!("{}%", fixed(x * 100.0, prec))
}

/// `repr(x)` / `str(x)` of a Python float (shortest round-trip digits; scientific outside 1e-4..1e16).
pub fn repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let e = format!("{:e}", x); // e.g. "-1.2345e-5", shortest round-trip digits
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let (sign, mant) = match mant.strip_prefix('-') {
        Some(m) => ("-", m),
        None => ("", mant),
    };
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    if (-4..16).contains(&exp) {
        let n = digits.len() as i32;
        let point = exp + 1; // digits before the decimal point
        let s = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point >= n {
            format!("{}{}.0", digits, "0".repeat((point - n) as usize))
        } else {
            format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
        };
        format!("{sign}{s}")
    } else {
        let m = if digits.len() > 1 { format!("{}.{}", &digits[..1], &digits[1..]) } else { digits.clone() };
        format!("{sign}{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

/// Python `round(x, nd)` for a float (correctly rounded decimal; no exact binary ties at these precisions).
pub fn round_to(x: f64, nd: usize) -> f64 {
    fixed(x, nd).parse().unwrap()
}

/// Python `float(s)`
pub fn parse_float(s: &str) -> Option<f64> {
    s.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_python() {
        assert_eq!(repr(0.0), "0.0");
        assert_eq!(repr(0.03), "0.03");
        assert_eq!(repr(0.0325), "0.0325");
        assert_eq!(repr(1e-5), "1e-05");
        assert_eq!(repr(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(repr(3.0), "3.0");
        assert_eq!(repr(1e16), "1e+16");
        assert_eq!(repr(1e15), "1000000000000000.0");
        assert_eq!(repr(0.0001), "0.0001");
        assert_eq!(repr(111105.99008746355), "111105.99008746355");
        assert_eq!(repr(1.2345678901234568e17), "1.2345678901234568e+17");
        assert_eq!(fixed_grouped(1234567.891, 2), "1,234,567.89");
        assert_eq!(fixed_grouped(-999.5, 0), "-1,000");
        assert_eq!(fixed_grouped(12.5, 0), "12");
        assert_eq!(int_grouped(1250), "1,250");
        assert_eq!(int_grouped(-1234567), "-1,234,567");
        assert_eq!(pct(0.0325, 2), "3.25%");
        assert_eq!(pct(0.7, 0), "70%");
        assert_eq!(round_to(0.0325000000001, 6), 0.0325);
    }
}
