//! Binary cache pins.
//!
//! ## Pin Naming
//!
//! Pin names can be up to 50 characters long and can only consist of
//! ASCII alphanumeric characters (A-Za-z0-9), dashes ('-'), and
//! underscores ('_'). They must also start with an alphanumeric character
//! (e.g., "_pin" is _not_ a valid pin name).
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use lazy_static::lazy_static;
use regex::Regex;
use serde::{de, Deserialize, Serialize};

use crate::error::{AtticError, AtticResult};

lazy_static! {
    static ref PIN_NAME_REGEX: Regex = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9-_]{0,49}$").unwrap();
}

/// The name of a cache pin.
///
/// Names can only consist of ASCII alphanumeric characters (A-Za-z0-9),
/// dashes ('-'), and underscores ('_').
#[derive(Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(transparent)]
pub struct PinName(#[serde(deserialize_with = "PinName::deserialize")] String);

impl PinName {
    /// Creates a pin name from a String.
    pub fn new(name: String) -> AtticResult<Self> {
        validate_pin_name(&name)?;
        Ok(Self(name))
    }

    /// Creates a pin name from a String, without checking its validity.
    ///
    /// # Safety
    ///
    /// The caller must make sure that it is of expected length and format.
    #[allow(unsafe_code)]
    pub unsafe fn new_unchecked(name: String) -> Self {
        Self(name)
    }

    /// Returns the string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Deserializes a potentially-invalid pin name.
    fn deserialize<'de, D>(deserializer: D) -> Result<String, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        use de::Error;
        String::deserialize(deserializer).and_then(|s| {
            validate_pin_name(&s).map_err(|e| Error::custom(e.to_string()))?;
            Ok(s)
        })
    }
}

impl FromStr for PinName {
    type Err = AtticError;

    fn from_str(name: &str) -> AtticResult<Self> {
        Self::new(name.to_owned())
    }
}

impl Display for PinName {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn validate_pin_name(name: &str) -> AtticResult<()> {
    if PIN_NAME_REGEX.is_match(name) {
        Ok(())
    } else {
        Err(AtticError::InvalidPinName {
            name: name.to_owned(),
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    macro_rules! pin {
        ($n:expr) => {
            PinName::new($n.to_string()).unwrap()
        };
    }

    pub(crate) use pin;

    #[test]
    fn test_pin_name() {
        let names = vec!["valid-name", "Another_Valid_Name", "plan9"];

        for name in names {
            assert_eq!(name, PinName::new(name.to_string()).unwrap().as_str());

            assert_eq!(
                name,
                serde_json::from_str::<PinName>(&format!("\"{}\"", name))
                    .unwrap()
                    .as_str(),
            );
        }

        let bad_names = vec![
            "",
            "not a valid name",
            "team-*",
            "这布盒里.webp",
            "-ers",
            "and-you-can-have-it-all-my-empire-of-dirt-i-will-let-you-down-i-will-make-you-hurt",
            "username+pin",
        ];

        for name in bad_names {
            PinName::new(name.to_string()).unwrap_err();
            serde_json::from_str::<PinName>(&format!("\"{}\"", name)).unwrap_err();
        }
    }
}
