use std::str;

use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref AUTHORIZATION_REGEX: Regex =
        Regex::new(r"^(?i)((?P<bearer>bearer)|(?P<basic>basic))(?-i) (?P<rest>(.*))$").unwrap();
}

/// Extracts the JWT from an Authorization header.
pub fn parse_authorization_header(authorization: &str) -> Option<String> {
    let captures = AUTHORIZATION_REGEX.captures(authorization)?;
    let rest = captures.name("rest").unwrap().as_str();

    if captures.name("bearer").is_some() {
        // Bearer token
        Some(rest.to_string())
    } else {
        // Basic auth
        let bytes = base64::decode(rest).ok()?;

        let user_pass = str::from_utf8(&bytes).ok()?;
        let colon = user_pass.find(':')?;
        let pass = &user_pass[colon + 1..];

        Some(pass.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_authorization_header() {
        assert_eq!(
            "somepass",
            parse_authorization_header("Basic c29tZXVzZXI6c29tZXBhc3M=").unwrap(),
        );

        assert_eq!(
            "somepass",
            parse_authorization_header("baSIC c29tZXVzZXI6c29tZXBhc3M=").unwrap(),
        );

        assert_eq!(
            "some-token",
            parse_authorization_header("bearer some-token").unwrap(),
        );
    }
}
