//! Nix netrc files.
//!
//! We automatically edit the user's `netrc` to add cache server
//! tokens.
//!
//! This is a very naive implementation. The whole thing should be
//! refactored to be cleaner and operate on streams.

use std::collections::HashMap;
use std::fmt;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use xdg::BaseDirectories;

/// The permission the configuration file should have.
const FILE_MODE: u32 = 0o600;

#[derive(Debug)]
pub struct NixNetrc {
    /// Path to write the modified netrc back to.
    path: Option<PathBuf>,

    /// Machines in the netrc file.
    machines: HashMap<String, Machine>,
}

#[derive(Debug, PartialEq, Eq)]
struct Machine {
    /// A password.
    password: Option<String>,

    /// Any other tokens that we must preserve.
    ///
    /// We output in pairs when reserializing. Curl allows the key
    /// and value to be on different lines, but who knows about other
    /// implementations?
    other: Vec<String>,
}

impl NixNetrc {
    pub async fn load() -> Result<Self> {
        let nix_base = BaseDirectories::with_prefix("nix")?;
        let path = nix_base.place_config_file("netrc")?;

        let machines = if path.exists() {
            let content = fs::read_to_string(&path).await?;
            parse_machines(&content)?
        } else {
            HashMap::new()
        };

        Ok(Self {
            path: Some(path),
            machines,
        })
    }

    /// Returns the path to the netrc file.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Saves the modified configuration file.
    pub async fn save(&self) -> Result<()> {
        if let Some(path) = &self.path {
            let mut content = String::new();
            serialize_machines(&mut content, &self.machines)?;

            // This isn't atomic, so some other process might chmod it
            // to something else before we write. We don't handle this case.
            if path.exists() {
                let permissions = Permissions::from_mode(FILE_MODE);
                fs::set_permissions(path, permissions).await?;
            }

            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .mode(FILE_MODE)
                .open(path)
                .await?;

            file.write_all(content.as_bytes()).await?;
            Ok(())
        } else {
            Err(anyhow!("Don't know how to save the netrc"))
        }
    }

    /// Adds a token as a password.
    pub fn add_token(&mut self, machine: String, token: String) {
        if let Some(m) = self.machines.get_mut(&machine) {
            m.password = Some(token);
        } else {
            self.machines.insert(
                machine,
                Machine {
                    password: Some(token),
                    other: Vec::new(),
                },
            );
        }
    }
}

fn parse_machines(netrc: &str) -> Result<HashMap<String, Machine>> {
    let mut machines = HashMap::new();
    let mut cur_machine = None;

    let mut cur;
    let mut remaining = netrc;
    while !remaining.is_empty() {
        (cur, remaining) = get_next_token(remaining);

        match cur {
            "" => {
                break;
            }
            "default" => {
                if let Some((name, machine)) = cur_machine {
                    machines.insert(name, machine);
                }

                cur_machine = Some((
                    "".to_string(),
                    Machine {
                        password: None,
                        other: Vec::new(),
                    },
                ));
            }
            "machine" => {
                let (m_name, m_remaining) = get_next_token(remaining);
                remaining = m_remaining;

                if let Some((name, machine)) = cur_machine {
                    machines.insert(name, machine);
                }

                cur_machine = Some((
                    m_name.to_string(),
                    Machine {
                        password: None,
                        other: Vec::new(),
                    },
                ));
            }
            "password" => {
                let (m_password, m_remaining) = get_next_token(remaining);
                remaining = m_remaining;

                if let Some((_, ref mut machine)) = &mut cur_machine {
                    machine.password = Some(m_password.to_string());
                } else {
                    return Err(anyhow!("Password field outside a machine block"));
                }
            }
            tok => {
                if let Some((_, ref mut machine)) = &mut cur_machine {
                    machine.other.push(tok.to_string());
                } else {
                    return Err(anyhow!("Unknown token {} outside a machine block", tok));
                }
            }
        }
    }

    if let Some((name, machine)) = cur_machine {
        machines.insert(name, machine);
    }

    Ok(machines)
}

fn serialize_machines(w: &mut impl fmt::Write, machines: &HashMap<String, Machine>) -> Result<()> {
    for (name, machine) in machines.iter() {
        if name.is_empty() {
            writeln!(w, "default")?;
        } else {
            writeln!(w, "machine {}", name)?;
        }

        if let Some(password) = &machine.password {
            writeln!(w, "password {}", password)?;
        }

        for chunk in machine.other.chunks(2) {
            writeln!(w, "{}", chunk.join(" "))?;
        }
    }

    Ok(())
}

fn get_next_token(s: &str) -> (&str, &str) {
    let s = strip_leading_whitespace(s);
    if let Some(idx) = s.find(|c| c == '\n' || c == ' ' || c == '\t') {
        (&s[..idx], strip_leading_whitespace(&s[idx + 1..]))
    } else {
        (s, "")
    }
}

fn strip_leading_whitespace(s: &str) -> &str {
    if let Some(idx) = s.find(|c| c != '\n' && c != ' ' && c != '\t') {
        &s[idx..]
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_netrc_strip() {
        assert_eq!("", strip_leading_whitespace("   "));
        assert_eq!("a", strip_leading_whitespace("   a"));
        assert_eq!("abc", strip_leading_whitespace("   \t\t\n\nabc"));
        assert_eq!("abc", strip_leading_whitespace("abc"));
    }

    #[test]
    fn test_netrc_tokenization() {
        assert_eq!(("", ""), get_next_token(""));
        assert_eq!(("", ""), get_next_token(" "));
        assert_eq!(("", ""), get_next_token("\n"));
        assert_eq!(("", ""), get_next_token("\t"));

        assert_eq!(("a", ""), get_next_token("a "));
        assert_eq!(("a", ""), get_next_token(" a"));
        assert_eq!(("a", ""), get_next_token("  a "));

        assert_eq!(("abc", ""), get_next_token("abc"));

        assert_eq!(("a", "b"), get_next_token("a b"));
        assert_eq!(("a", "b c"), get_next_token("a b c"));
        assert_eq!(("a", "b\nc"), get_next_token("a\nb\nc"));
        assert_eq!(("a", "b\nc"), get_next_token("a\tb\nc"));

        assert_eq!(("a", "b c"), get_next_token("a       b c"));
        assert_eq!(("a", "b\nc"), get_next_token("a\n\n\nb\nc"));
        assert_eq!(("a", "b\nc"), get_next_token("a\n\t\nb\nc"));
    }

    #[test]
    fn test_netrc_parse() {
        let machines = parse_machines(
            "default password hunter2 machine localhost login login password 114514",
        )
        .unwrap();
        eprintln!("{:#?}", machines);

        assert_eq!(Some("114514".to_string()), machines["localhost"].password);

        let mut serialized = String::new();
        serialize_machines(&mut serialized, &machines).unwrap();
        eprintln!("{}", serialized);

        let reparse = parse_machines(&serialized).unwrap();
        assert_eq!(machines, reparse);
    }
}
