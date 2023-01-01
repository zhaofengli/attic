//! Binary caches.
//!
//! ## Cache Naming
//!
//! Cache names can be up to 50 characters long and can only consist of
//! ASCII alphanumeric characters (A-Za-z0-9), dashes ('-'), underscores
//! ('_'), and plus signs ('+'). They must also start with an alphanumeric
//! character (e.g., "_cache" is _not_ a valid cache name).
//!
/// The plus sign is intended to be used as the delimiter between a
/// namespace and a user-given name (e.g., `zhaofengli+shared`).
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use lazy_static::lazy_static;
use regex::Regex;
use serde::{de, Deserialize, Serialize};
use wildmatch::WildMatch;

use crate::error::{AtticError, AtticResult};

/// The maximum allowable length of a cache name.
pub const MAX_NAME_LENGTH: usize = 50;

lazy_static! {
    static ref CACHE_NAME_REGEX: Regex = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9-_+]{0,49}$").unwrap();
    static ref CACHE_NAME_PATTERN_REGEX: Regex =
        Regex::new(r"^[A-Za-z0-9*][A-Za-z0-9-_+*]{0,49}$").unwrap();
}

/// The name of a binary cache.
///
/// Names can only consist of ASCII alphanumeric characters (A-Za-z0-9),
/// dashes ('-'), underscores ('_'), and plus signs ('+').
#[derive(Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(transparent)]
pub struct CacheName(#[serde(deserialize_with = "CacheName::deserialize")] String);

/// A pattern of cache names.
///
/// The keys in the custom JWT claim are patterns that can
/// be matched against cache names. Thus patterns can only be created
/// by trusted entities.
///
/// In addition to what's allowed in cache names, patterns can include
/// wildcards ('*') to enable a limited form of namespace-based access
/// control.
///
/// This is particularly useful in conjunction with the `cache_create`
/// permission which allows the user to autonomously create caches under
/// their own namespace (e.g., `zhaofengli+*`).
#[derive(Serialize, Clone, Debug)]
#[serde(transparent)]
pub struct CacheNamePattern {
    pattern: String,

    /// The pattern matcher.
    ///
    /// If None, then `pattern` itself will be used to match exactly.
    /// This is a special case for converting a CacheName to a
    /// CacheNamePattern.
    ///
    /// It's possible to combine the two structs into one, but the goal
    /// is to have strong delineation between them enforced by the type
    /// system (you can't attempt to call `matches` at all on a regular
    /// CacheName).
    #[serde(skip)]
    matcher: Option<WildMatch>,
}

impl CacheName {
    /// Creates a cache name from a String.
    pub fn new(name: String) -> AtticResult<Self> {
        validate_cache_name(&name, false)?;
        Ok(Self(name))
    }

    /// Returns the string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_string(&self) -> String {
        self.0.clone()
    }

    /// Returns the corresponding pattern that only matches this cache.
    pub fn to_pattern(&self) -> CacheNamePattern {
        CacheNamePattern {
            pattern: self.0.clone(),
            matcher: None,
        }
    }

    /// Deserializes a potentially-invalid cache name.
    fn deserialize<'de, D>(deserializer: D) -> Result<String, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        use de::Error;
        String::deserialize(deserializer).and_then(|s| {
            validate_cache_name(&s, false).map_err(|e| Error::custom(e.to_string()))?;
            Ok(s)
        })
    }
}

impl FromStr for CacheName {
    type Err = AtticError;

    fn from_str(name: &str) -> AtticResult<Self> {
        Self::new(name.to_owned())
    }
}

impl CacheNamePattern {
    /// Creates a cache name pattern from a String.
    pub fn new(pattern: String) -> AtticResult<Self> {
        validate_cache_name(&pattern, true)?;
        let matcher = WildMatch::new(&pattern);

        Ok(Self {
            pattern,
            matcher: Some(matcher),
        })
    }

    /// Tests if the pattern matches a name.
    pub fn matches(&self, name: &CacheName) -> bool {
        match &self.matcher {
            Some(matcher) => matcher.matches(name.as_str()),
            None => self.pattern == name.as_str(),
        }
    }
}

impl FromStr for CacheNamePattern {
    type Err = AtticError;

    fn from_str(pattern: &str) -> AtticResult<Self> {
        Self::new(pattern.to_owned())
    }
}

impl<'de> Deserialize<'de> for CacheNamePattern {
    /// Deserializes a potentially-invalid cache name pattern.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        use de::Error;
        let pattern = String::deserialize(deserializer).and_then(|s| {
            validate_cache_name(&s, true).map_err(|e| Error::custom(e.to_string()))?;
            Ok(s)
        })?;

        let matcher = WildMatch::new(&pattern);

        Ok(Self {
            pattern,
            matcher: Some(matcher),
        })
    }
}

impl Hash for CacheNamePattern {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.pattern.hash(state);
    }
}

impl PartialEq for CacheNamePattern {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern
    }
}

impl Eq for CacheNamePattern {}

fn validate_cache_name(name: &str, allow_wildcards: bool) -> AtticResult<()> {
    let valid = if allow_wildcards {
        CACHE_NAME_PATTERN_REGEX.is_match(name)
    } else {
        CACHE_NAME_REGEX.is_match(name)
    };

    if valid {
        Ok(())
    } else {
        Err(AtticError::InvalidCacheName {
            name: name.to_owned(),
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    macro_rules! cache {
        ($n:expr) => {
            CacheName::new($n.to_string()).unwrap()
        };
    }

    pub(crate) use cache;

    #[test]
    fn test_cache_name() {
        let names = vec![
            "valid-name",
            "Another_Valid_Name",
            "plan9",
            "username+cache",
        ];

        for name in names {
            assert_eq!(name, CacheName::new(name.to_string()).unwrap().as_str());

            assert_eq!(
                name,
                serde_json::from_str::<CacheName>(&format!("\"{}\"", name))
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
        ];

        for name in bad_names {
            CacheName::new(name.to_string()).unwrap_err();
            serde_json::from_str::<CacheName>(&format!("\"{}\"", name)).unwrap_err();
        }
    }

    #[test]
    fn test_cache_name_pattern() {
        let pattern = CacheNamePattern::new("team-*".to_string()).unwrap();
        assert!(pattern.matches(&cache! { "team-" }));
        assert!(pattern.matches(&cache! { "team-abc" }));
        assert!(!pattern.matches(&cache! { "abc-team" }));

        let pattern = CacheNamePattern::new("no-wildcard".to_string()).unwrap();
        assert!(pattern.matches(&cache! { "no-wildcard" }));
        assert!(!pattern.matches(&cache! { "no-wildcard-xxx" }));
        assert!(!pattern.matches(&cache! { "xxx-no-wildcard" }));

        let pattern = CacheNamePattern::new("*".to_string()).unwrap();
        assert!(pattern.matches(&cache! { "literally-anything" }));

        CacheNamePattern::new("*-but-normal-restrictions-still-apply!!!".to_string()).unwrap_err();

        // eq
        let pattern1 = CacheNamePattern::new("same-pattern".to_string()).unwrap();
        let pattern2 = CacheNamePattern::new("same-pattern".to_string()).unwrap();
        assert_eq!(pattern1, pattern2);
        assert_ne!(pattern, pattern1);
    }
}
