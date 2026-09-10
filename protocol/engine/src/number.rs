use std::{cmp::Ordering, hash::Hash};

use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};

#[derive(Clone, Debug)]
pub struct JsonNumber {
    token: Box<str>,
    value: Decimal,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
enum Decimal {
    Zero,
    Nonzero {
        negative: bool,
        digits: Box<str>,
        exponent: BigInt,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidNumber {
    pub byte: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsignedConversionError {
    Negative,
    Fractional,
    OutOfRange,
}

impl JsonNumber {
    /// Parse one complete JSON number, retaining its exact source spelling.
    pub fn parse(token: &str) -> Result<Self, InvalidNumber> {
        let bytes = token.as_bytes();
        let mut at = 0;
        let negative = bytes.first() == Some(&b'-');
        if negative {
            at += 1;
        }
        let integer_start = at;
        match bytes.get(at) {
            Some(b'0') => at += 1,
            Some(b'1'..=b'9') => {
                at += 1;
                while bytes.get(at).is_some_and(u8::is_ascii_digit) {
                    at += 1;
                }
            }
            _ => return Err(InvalidNumber { byte: at }),
        }
        let integer_end = at;
        let mut fraction_start = at;
        if bytes.get(at) == Some(&b'.') {
            at += 1;
            fraction_start = at;
            while bytes.get(at).is_some_and(u8::is_ascii_digit) {
                at += 1;
            }
            if at == fraction_start {
                return Err(InvalidNumber { byte: at });
            }
        }
        let fraction_end = at;
        let mut exponent = BigInt::zero();
        if matches!(bytes.get(at), Some(b'e' | b'E')) {
            at += 1;
            let exponent_start = at;
            if matches!(bytes.get(at), Some(b'+' | b'-')) {
                at += 1;
            }
            let exponent_digits = at;
            while bytes.get(at).is_some_and(u8::is_ascii_digit) {
                at += 1;
            }
            if at == exponent_digits {
                return Err(InvalidNumber { byte: at });
            }
            exponent = token[exponent_start..at]
                .parse()
                .map_err(|_| InvalidNumber {
                    byte: exponent_start,
                })?;
        }
        if at != bytes.len() {
            return Err(InvalidNumber { byte: at });
        }

        let coefficient = format!(
            "{}{}",
            &token[integer_start..integer_end],
            &token[fraction_start..fraction_end]
        );
        let significant = coefficient.trim_start_matches('0');
        let value = if significant.is_empty() {
            Decimal::Zero
        } else {
            let digits = significant.trim_end_matches('0');
            exponent -= BigInt::from(fraction_end - fraction_start);
            exponent += BigInt::from(significant.len() - digits.len());
            Decimal::Nonzero {
                negative,
                digits: digits.into(),
                exponent,
            }
        };
        Ok(Self {
            token: token.into(),
            value,
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn is_integer(&self) -> bool {
        match &self.value {
            Decimal::Zero => true,
            Decimal::Nonzero { exponent, .. } => !exponent.is_negative(),
        }
    }

    pub fn as_unsigned(&self, maximum: u64) -> Result<u64, UnsignedConversionError> {
        let Decimal::Nonzero {
            negative,
            digits,
            exponent,
        } = &self.value
        else {
            return Ok(0);
        };
        if *negative {
            return Err(UnsignedConversionError::Negative);
        }
        if exponent.is_negative() {
            return Err(UnsignedConversionError::Fractional);
        }
        if exponent + BigInt::from(digits.len()) > BigInt::from(20u8) {
            return Err(UnsignedConversionError::OutOfRange);
        }
        let mut result = digits
            .parse::<u64>()
            .map_err(|_| UnsignedConversionError::OutOfRange)?;
        let zeros = exponent
            .to_u32()
            .ok_or(UnsignedConversionError::OutOfRange)?;
        for _ in 0..zeros {
            result = result
                .checked_mul(10)
                .ok_or(UnsignedConversionError::OutOfRange)?;
        }
        if result > maximum {
            return Err(UnsignedConversionError::OutOfRange);
        }
        Ok(result)
    }
}

impl PartialEq for JsonNumber {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl Eq for JsonNumber {}

impl Hash for JsonNumber {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl PartialOrd for JsonNumber {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for JsonNumber {
    fn cmp(&self, other: &Self) -> Ordering {
        let sign = |value: &Decimal| match value {
            Decimal::Zero => 0i8,
            Decimal::Nonzero { negative: true, .. } => -1,
            Decimal::Nonzero {
                negative: false, ..
            } => 1,
        };
        let sign_order = sign(&self.value).cmp(&sign(&other.value));
        if sign_order != Ordering::Equal {
            return sign_order;
        }
        let (
            Decimal::Nonzero {
                negative,
                digits: left,
                exponent: left_exp,
            },
            Decimal::Nonzero {
                digits: right,
                exponent: right_exp,
                ..
            },
        ) = (&self.value, &other.value)
        else {
            return Ordering::Equal;
        };
        let magnitude =
            (left_exp + BigInt::from(left.len())).cmp(&(right_exp + BigInt::from(right.len())));
        let absolute = magnitude.then_with(|| {
            left.bytes()
                .chain(std::iter::repeat(b'0'))
                .zip(right.bytes().chain(std::iter::repeat(b'0')))
                .take(left.len().max(right.len()))
                .map(|(a, b)| a.cmp(&b))
                .find(|order| *order != Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        });
        if *negative {
            absolute.reverse()
        } else {
            absolute
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_rational::BigRational;
    use std::hash::{DefaultHasher, Hasher};

    fn number(token: &str) -> JsonNumber {
        JsonNumber::parse(token).unwrap()
    }

    #[test]
    fn complete_number_grammar_and_original_spelling() {
        for token in [
            "0",
            "-0",
            "1.0",
            "1E0",
            "1e+0",
            "-0.001e-02",
            "0e999999999999999999999999",
        ] {
            assert_eq!(number(token).token(), token);
        }
        for token in [
            "", "-", "+1", "00", "01", "-.1", ".1", "1.", "1e", "1e+", "1e-", "1E+ 2", " 1", "1 ",
            "1NaN", "NaN", "Infinity", "１", "1\0",
        ] {
            assert!(JsonNumber::parse(token).is_err(), "accepted {token:?}");
        }
    }

    #[test]
    fn equal_values_hash_equally_without_erasing_source_tokens() {
        for tokens in [
            ["1", "1.0", "1e0", "0.100e1"],
            ["-0", "0", "0.00", "-0e999999999999999999999999"],
        ] {
            let values = tokens.map(number);
            for value in &values {
                assert_eq!(value, &values[0]);
                assert_eq!(value.cmp(&values[0]), Ordering::Equal);
                let hash = |v: &JsonNumber| {
                    let mut h = DefaultHasher::new();
                    v.hash(&mut h);
                    h.finish()
                };
                assert_eq!(hash(value), hash(&values[0]));
            }
        }
    }

    #[test]
    fn exact_counts_and_huge_exponents_never_round_or_expand() {
        let safe = 9_007_199_254_740_991;
        assert_eq!(number("1.0").as_unsigned(safe), Ok(1));
        assert_eq!(number("1e0").as_unsigned(safe), Ok(1));
        assert_eq!(number("9007199254740991").as_unsigned(safe), Ok(safe));
        assert_eq!(
            number("9007199254740991.1").as_unsigned(safe),
            Err(UnsignedConversionError::Fractional)
        );
        assert_eq!(
            number("9007199254740992").as_unsigned(safe),
            Err(UnsignedConversionError::OutOfRange)
        );
        assert_eq!(
            number("18446744073709551615").as_unsigned(u64::MAX),
            Ok(u64::MAX)
        );
        assert_eq!(
            number("18446744073709551616").as_unsigned(u64::MAX),
            Err(UnsignedConversionError::OutOfRange)
        );
        assert!(!number("1e-1000001").is_integer());
        assert!(number("1e1000001").is_integer());
        let huge = number("1e999999999999999999999999999999999999999");
        assert!(huge > number("9e999999999999999999999999999999999999998"));
        assert!(huge > number("0.5"));
    }

    #[test]
    fn decimal_predicates_agree_with_independent_small_rationals() {
        let mut cases = Vec::new();
        for coefficient in -20i64..=20 {
            for exponent in -3i32..=3 {
                let value = number(&format!("{coefficient}e{exponent}"));
                let factor = BigInt::from(10u64.pow(exponent.unsigned_abs()));
                let rational = if exponent >= 0 {
                    BigRational::from_integer(BigInt::from(coefficient) * factor)
                } else {
                    BigRational::new(BigInt::from(coefficient), factor)
                };
                assert_eq!(value.is_integer(), rational.is_integer());
                cases.push((value, rational));
            }
        }
        for (left, a) in &cases {
            for (right, b) in &cases {
                assert_eq!(
                    left.cmp(right),
                    a.cmp(b),
                    "{} vs {}",
                    left.token(),
                    right.token()
                );
                assert_eq!(left == right, a == b);
            }
        }
    }
}
