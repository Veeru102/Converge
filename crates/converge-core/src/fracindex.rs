//! Fractional indexing keys for z-order.
//!
//! Port of the rocicorp `fractional-indexing` algorithm (base-62 alphabet,
//! variable-length integer part, no trailing zeros in the fractional part).
//! Keys compare by plain byte order. `between(a, b)` always yields a key
//! strictly between its arguments; concurrent equal keys are tie-broken by
//! `ObjectId` at render time.
//!
//! The TypeScript engine implements the same algorithm; `fixtures/fracindex.json`
//! pins both to identical outputs.

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const SMALLEST_INTEGER: &str = "A00000000000000000000000000";

/// Key used for the first object in an empty ordering.
pub const FIRST: &str = "a0";

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum FracError {
    #[error("invalid order key {0:?}")]
    InvalidKey(String),
    #[error("keys out of order: {0:?} >= {1:?}")]
    OutOfOrder(String, String),
    #[error("key range exhausted")]
    Exhausted,
}

fn digit_index(c: u8) -> Option<usize> {
    DIGITS.iter().position(|&d| d == c)
}

/// Midpoint between fractional strings `a` and `b` (`b == None` means 1.0).
/// Both must be free of trailing '0'; `a` may be empty.
fn midpoint(a: &str, b: Option<&str>) -> String {
    let ab = a.as_bytes();
    if let Some(b) = b {
        let bb = b.as_bytes();
        debug_assert!(ab < bb, "midpoint precondition {a:?} < {b:?}");
        let mut n = 0;
        while n < bb.len() && ab.get(n).copied().unwrap_or(b'0') == bb[n] {
            n += 1;
        }
        if n > 0 {
            let rest_a = std::str::from_utf8(&ab[n.min(ab.len())..]).unwrap();
            return format!("{}{}", &b[..n], midpoint(rest_a, Some(&b[n..])));
        }
    }
    let digit_a = ab.first().map(|&c| digit_index(c).unwrap()).unwrap_or(0);
    let digit_b = b
        .and_then(|b| b.as_bytes().first())
        .map(|&c| digit_index(c).unwrap())
        .unwrap_or(DIGITS.len());
    if digit_b - digit_a > 1 {
        let mid = (digit_a + digit_b).div_ceil(2); // == round(0.5 * (a + b))
        return (DIGITS[mid] as char).to_string();
    }
    match b {
        Some(b) if b.len() > 1 => b[..1].to_string(),
        _ => {
            let rest_a = if ab.is_empty() { "" } else { &a[1..] };
            format!("{}{}", DIGITS[digit_a] as char, midpoint(rest_a, None))
        }
    }
}

fn integer_length(head: u8) -> Option<usize> {
    match head {
        b'a'..=b'z' => Some((head - b'a') as usize + 2),
        b'A'..=b'Z' => Some((b'Z' - head) as usize + 2),
        _ => None,
    }
}

fn integer_part(key: &str) -> Result<&str, FracError> {
    let head = *key
        .as_bytes()
        .first()
        .ok_or_else(|| FracError::InvalidKey(key.into()))?;
    let len = integer_length(head).ok_or_else(|| FracError::InvalidKey(key.into()))?;
    if len > key.len() {
        return Err(FracError::InvalidKey(key.into()));
    }
    Ok(&key[..len])
}

/// Validate a key's shape.
pub fn validate(key: &str) -> Result<(), FracError> {
    if key == SMALLEST_INTEGER {
        return Err(FracError::InvalidKey(key.into()));
    }
    if !key.bytes().all(|c| digit_index(c).is_some()) {
        return Err(FracError::InvalidKey(key.into()));
    }
    let i = integer_part(key)?;
    let f = &key[i.len()..];
    if f.ends_with('0') {
        return Err(FracError::InvalidKey(key.into()));
    }
    Ok(())
}

fn increment_integer(x: &str) -> Result<Option<String>, FracError> {
    let bytes = x.as_bytes();
    let head = bytes[0];
    let mut digs: Vec<u8> = bytes[1..].to_vec();
    let mut carry = true;
    for d in digs.iter_mut().rev() {
        if !carry {
            break;
        }
        let i = digit_index(*d).unwrap() + 1;
        if i == DIGITS.len() {
            *d = b'0';
        } else {
            *d = DIGITS[i];
            carry = false;
        }
    }
    if carry {
        if head == b'Z' {
            return Ok(Some("a0".into()));
        }
        if head == b'z' {
            return Ok(None);
        }
        let h = head + 1;
        if h > b'a' {
            digs.push(b'0');
        } else {
            digs.pop();
        }
        let mut s = String::with_capacity(digs.len() + 1);
        s.push(h as char);
        s.push_str(std::str::from_utf8(&digs).unwrap());
        return Ok(Some(s));
    }
    let mut s = String::with_capacity(digs.len() + 1);
    s.push(head as char);
    s.push_str(std::str::from_utf8(&digs).unwrap());
    Ok(Some(s))
}

fn decrement_integer(x: &str) -> Result<Option<String>, FracError> {
    let bytes = x.as_bytes();
    let head = bytes[0];
    let mut digs: Vec<u8> = bytes[1..].to_vec();
    let mut borrow = true;
    for d in digs.iter_mut().rev() {
        if !borrow {
            break;
        }
        let i = digit_index(*d).unwrap();
        if i == 0 {
            *d = DIGITS[DIGITS.len() - 1];
        } else {
            *d = DIGITS[i - 1];
            borrow = false;
        }
    }
    if borrow {
        if head == b'a' {
            return Ok(Some(format!("Z{}", DIGITS[DIGITS.len() - 1] as char)));
        }
        if head == b'A' {
            return Ok(None);
        }
        let h = head - 1;
        if h < b'Z' {
            digs.push(DIGITS[DIGITS.len() - 1]);
        } else {
            digs.pop();
        }
        let mut s = String::with_capacity(digs.len() + 1);
        s.push(h as char);
        s.push_str(std::str::from_utf8(&digs).unwrap());
        return Ok(Some(s));
    }
    let mut s = String::with_capacity(digs.len() + 1);
    s.push(head as char);
    s.push_str(std::str::from_utf8(&digs).unwrap());
    Ok(Some(s))
}

/// A key strictly between `a` and `b`. `None` means the open end.
pub fn between(a: Option<&str>, b: Option<&str>) -> Result<String, FracError> {
    if let Some(a) = a {
        validate(a)?;
    }
    if let Some(b) = b {
        validate(b)?;
    }
    if let (Some(a), Some(b)) = (a, b) {
        if a >= b {
            return Err(FracError::OutOfOrder(a.into(), b.into()));
        }
    }
    match (a, b) {
        (None, None) => Ok(FIRST.to_string()),
        (None, Some(b)) => {
            let ib = integer_part(b)?;
            let fb = &b[ib.len()..];
            if ib == SMALLEST_INTEGER {
                return Ok(format!("{ib}{}", midpoint("", Some(fb))));
            }
            if ib < b {
                return Ok(ib.to_string());
            }
            decrement_integer(ib)?.ok_or(FracError::Exhausted)
        }
        (Some(a), None) => {
            let ia = integer_part(a)?;
            let fa = &a[ia.len()..];
            match increment_integer(ia)? {
                Some(i) => Ok(i),
                None => Ok(format!("{ia}{}", midpoint(fa, None))),
            }
        }
        (Some(a), Some(b)) => {
            let ia = integer_part(a)?;
            let fa = &a[ia.len()..];
            let ib = integer_part(b)?;
            let fb = &b[ib.len()..];
            if ia == ib {
                return Ok(format!("{ia}{}", midpoint(fa, Some(fb))));
            }
            let i = increment_integer(ia)?.ok_or(FracError::Exhausted)?;
            if i.as_str() < b {
                Ok(i)
            } else {
                Ok(format!("{ia}{}", midpoint(fa, None)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_vectors() {
        // Vectors from the reference implementation's test-suite.
        let cases: &[(Option<&str>, Option<&str>, &str)] = &[
            (None, None, "a0"),
            (None, Some("a0"), "Zz"),
            (None, Some("Zz"), "Zy"),
            (Some("a0"), None, "a1"),
            (Some("a1"), None, "a2"),
            (Some("a0"), Some("a1"), "a0V"),
            (Some("a1"), Some("a2"), "a1V"),
            (Some("a0V"), Some("a1"), "a0l"),
            (Some("Zz"), Some("a0"), "ZzV"),
            (Some("Zz"), Some("a1"), "a0"),
            (None, Some("Y00"), "Xzzz"),
            (Some("bzz"), None, "c000"),
            (Some("a0"), Some("a0V"), "a0G"),
            (Some("a0"), Some("a0G"), "a08"),
            (Some("b125"), Some("b129"), "b127"),
            (Some("a0"), Some("a1V"), "a1"),
            (Some("Zz"), Some("a01"), "a0"),
            (None, Some("a0V"), "a0"),
            (None, Some("b999"), "b99"),
            (
                Some("zzzzzzzzzzzzzzzzzzzzzzzzzzz"),
                None,
                "zzzzzzzzzzzzzzzzzzzzzzzzzzzV",
            ),
        ];
        for (a, b, want) in cases {
            assert_eq!(
                between(*a, *b).as_deref(),
                Ok(*want),
                "between({a:?}, {b:?})"
            );
        }
        assert!(between(None, Some("A00000000000000000000000000")).is_err());
        assert!(between(Some("a00"), None).is_err());
        assert!(between(Some("a1"), Some("a0")).is_err());
    }

    #[test]
    fn between_is_strictly_between() {
        let mut keys = vec![between(None, None).unwrap()];
        for i in 0..300 {
            let k = match i % 3 {
                0 => between(keys.last().map(String::as_str), None).unwrap(),
                1 => between(None, Some(keys[0].as_str())).unwrap(),
                _ => {
                    let m = keys.len() / 2;
                    between(Some(keys[m - 1].as_str()), Some(keys[m].as_str())).unwrap()
                }
            };
            match i % 3 {
                0 => keys.push(k),
                1 => keys.insert(0, k),
                _ => keys.insert(keys.len() / 2, k),
            }
            for w in keys.windows(2) {
                assert!(w[0] < w[1], "{:?} !< {:?}", w[0], w[1]);
            }
        }
    }
}
