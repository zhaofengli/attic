//! Nix configuration files.
//!
//! We automatically edit the user's `nix.conf` to add new
//! binary caches while trying to keep the formatting intact.

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use lazy_static::lazy_static;
use regex::Regex;
use tokio::fs;
use xdg::BaseDirectories;

lazy_static! {
    static ref COMMENT_LINE: Regex = {
        Regex::new(r"^\s*(#.*)?$").unwrap()
    };

    static ref KV_LINE: Regex = {
        // I know what you are thinking, but...
        // `key=val` is not valid, and `🔥🔥🔥very=WILD=key🔥🔥🔥 = value` is perfectly valid :)
        // Also, despite syntax highlighting of some editors, backslashes do _not_ escape the comment character.
        Regex::new(r"^(?P<whitespace_s>\s*)(?P<key>[^\s]+)(?P<whitespace_l>\s+)=(?P<whitespace_r>\s+)(?P<value>[^#]+)(?P<comment>#.*)?$").unwrap()
    };
}

/// The server of cache.nixos.org.
const CACHE_NIXOS_ORG_SUBSTITUTER: &str = "https://cache.nixos.org";

/// The public key of cache.nixos.org.
const CACHE_NIXOS_ORG_KEY: &str = "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=";

#[derive(Debug)]
pub struct NixConfig {
    /// Path to write the modified configuration back to.
    path: Option<PathBuf>,

    /// Configuration lines.
    lines: Vec<Line>,
}

/// A line in the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    Comment(String),
    KV {
        key: String,
        value: String,
        whitespace_s: String,
        whitespace_l: String,
        whitespace_r: String,
        comment: Option<String>,
    },
}

impl NixConfig {
    pub async fn load() -> Result<Self> {
        let nix_base = BaseDirectories::with_prefix("nix")?;
        let path = nix_base.place_config_file("nix.conf")?;

        let lines = if path.exists() {
            let content = fs::read_to_string(&path).await?;
            Line::from_lines(&content)?
        } else {
            Vec::new()
        };

        Ok(Self {
            path: Some(path),
            lines,
        })
    }

    /// Saves the modified configuration file.
    pub async fn save(&self) -> Result<()> {
        if let Some(path) = &self.path {
            fs::write(path, self.to_string()).await?;
            Ok(())
        } else {
            Err(anyhow!("Don't know how to save the nix.conf"))
        }
    }

    /// Reserialize the configuration back to a string.
    pub fn to_string(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Adds a new substituter.
    pub fn add_substituter(&mut self, substituter: &str) {
        self.prepend_to_list("substituters", substituter, CACHE_NIXOS_ORG_SUBSTITUTER);
    }

    /// Adds a new trusted public key.
    pub fn add_trusted_public_key(&mut self, public_key: &str) {
        self.prepend_to_list("trusted-public-keys", public_key, CACHE_NIXOS_ORG_KEY);
    }

    /// Sets the netrc-file config.
    pub fn set_netrc_file(&mut self, path: &str) {
        if let Some(kv) = self.find_key("netrc-file") {
            if let Line::KV { ref mut value, .. } = kv {
                *value = path.to_string();
            }
        } else {
            self.lines
                .push(Line::kv("netrc-file".to_string(), path.to_string()));
        }
    }

    fn prepend_to_list(&mut self, key: &str, value: &str, default_tail: &str) {
        if let Some(kv) = self.find_key(key) {
            if let Line::KV {
                value: ref mut list,
                ..
            } = kv
            {
                if !list.split(' ').any(|el| el == value) {
                    *list = format!("{value} {list}");
                }
                return;
            }
            unreachable!();
        } else {
            let list = format!("{value} {default_tail}");
            self.lines.push(Line::kv(key.to_string(), list));
        }
    }

    fn find_key(&mut self, key: &str) -> Option<&mut Line> {
        self.lines.iter_mut().find(|l| {
            if let Line::KV { key: k, .. } = l {
                k == key
            } else {
                false
            }
        })
    }
}

impl Line {
    fn from_lines(s: &str) -> Result<Vec<Self>> {
        let mut lines: Vec<Line> = Vec::new();

        for line in s.lines() {
            lines.push(Line::from_str(line)?);
        }

        Ok(lines)
    }

    fn from_str(line: &str) -> Result<Self> {
        if COMMENT_LINE.is_match(line) {
            return Ok(Self::Comment(line.to_string()));
        }

        if let Some(matches) = KV_LINE.captures(line) {
            return Ok(Self::KV {
                key: matches.name("key").unwrap().as_str().to_owned(),
                value: matches.name("value").unwrap().as_str().to_owned(),
                whitespace_s: matches.name("whitespace_s").unwrap().as_str().to_owned(),
                whitespace_l: matches.name("whitespace_l").unwrap().as_str().to_owned(),
                whitespace_r: matches.name("whitespace_r").unwrap().as_str().to_owned(),
                comment: matches.name("comment").map(|s| s.as_str().to_owned()),
            });
        }

        Err(anyhow!("Line \"{}\" isn't valid", line))
    }

    fn to_string(&self) -> String {
        match self {
            Self::Comment(l) => l.clone(),
            Self::KV {
                key,
                value,
                whitespace_s,
                whitespace_l,
                whitespace_r,
                comment,
            } => {
                let cmt = comment.as_deref().unwrap_or("");
                format!("{whitespace_s}{key}{whitespace_l}={whitespace_r}{value}{cmt}")
            }
        }
    }

    fn kv(key: String, value: String) -> Self {
        Self::KV {
            key,
            value,
            whitespace_s: String::new(),
            whitespace_l: " ".to_string(),
            whitespace_r: " ".to_string(),
            comment: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nix_config_parse_line() {
        assert_eq!(
            Line::from_str("# some comment").unwrap(),
            Line::Comment("# some comment".to_string()),
        );

        assert_eq!(
            Line::from_str("    # some indented comment").unwrap(),
            Line::Comment("    # some indented comment".to_string()),
        );

        assert_eq!(
            Line::from_str(" 		 		 	 		  	 	 		 				 			 			").unwrap(),
            Line::Comment(" 		 		 	 		  	 	 		 				 			 			".to_string()),
        );

        assert_eq!(
            Line::from_str("key = value").unwrap(),
            Line::KV {
                key: "key".to_string(),
                value: "value".to_string(),
                whitespace_s: "".to_string(),
                whitespace_l: " ".to_string(),
                whitespace_r: " ".to_string(),
                comment: None,
            }
        );

        assert_eq!(
            Line::from_str("	 🔥🔥🔥very=WILD=key🔥🔥🔥 =	value = #comment").unwrap(),
            Line::KV {
                key: "🔥🔥🔥very=WILD=key🔥🔥🔥".to_string(),
                value: "value = ".to_string(),
                whitespace_s: "\t ".to_string(),
                whitespace_l: " ".to_string(),
                whitespace_r: "\t".to_string(),
                comment: Some("#comment".to_string()),
            }
        );
    }

    #[test]
    fn test_nix_config_line_roundtrip() {
        let cases = [
            "# some comment",
            "    # some indented comment",
            " 		 		 	 		  	 	 		 				 			 			",
            "key = value",
            "	 🔥🔥🔥very=WILD=key🔥🔥🔥 =	value = #comment",
        ];

        for case in cases {
            let line = Line::from_str(case).unwrap();
            assert_eq!(case, line.to_string());
        }
    }
}
