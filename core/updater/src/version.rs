use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A release version `MAJOR.MINOR.PATCH`. Ordering is numeric, field by field.
///
/// Only the canonical spelling is accepted (no `+`, no leading zeros, no
/// suffixes), so one version has exactly one string form inside a signed
/// manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Version { major, minor, patch }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParseVersionError(pub String);

impl fmt::Display for ParseVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid version {:?}: expected MAJOR.MINOR.PATCH", self.0)
    }
}

impl std::error::Error for ParseVersionError {}

impl FromStr for Version {
    type Err = ParseVersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseVersionError(s.to_string());
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(err());
        }
        let mut nums = [0u32; 3];
        for (n, part) in nums.iter_mut().zip(&parts) {
            let canonical = !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'));
            if !canonical {
                return Err(err());
            }
            *n = part.parse().map_err(|_| err())?;
        }
        Ok(Version::new(nums[0], nums[1], nums[2]))
    }
}

impl Serialize for Version {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_canonical_form() {
        let v: Version = "1.20.3".parse().unwrap();
        assert_eq!(v, Version::new(1, 20, 3));
        assert_eq!(v.to_string(), "1.20.3");
        assert_eq!("0.0.0".parse::<Version>().unwrap(), Version::new(0, 0, 0));
    }

    #[test]
    fn rejects_non_canonical_spellings() {
        for bad in ["", "1", "1.2", "1.2.3.4", "+1.2.3", "01.2.3", "1.2.3-beta", "1..3", "a.b.c", " 1.2.3", "1.2.99999999999"] {
            assert!(bad.parse::<Version>().is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn orders_numerically_not_lexically() {
        let v = |s: &str| s.parse::<Version>().unwrap();
        assert!(v("1.10.0") > v("1.9.9"));
        assert!(v("2.0.0") > v("1.99.99"));
        assert!(v("1.2.10") > v("1.2.9"));
    }
}
