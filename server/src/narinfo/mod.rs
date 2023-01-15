//! NAR info.
//!
//! ## `.narinfo` format
//!
//! An example of [a valid
//! .narinfo](https://cache.nixos.org/p4pclmv1gyja5kzc26npqpia1qqxrf0l.narinfo)
//! signed by https://cache.nixos.org:
//!
//! ```text
//! StorePath: /nix/store/p4pclmv1gyja5kzc26npqpia1qqxrf0l-ruby-2.7.3
//! URL: nar/1w1fff338fvdw53sqgamddn1b2xgds473pv6y13gizdbqjv4i5p3.nar.xz
//! Compression: xz
//! FileHash: sha256:1w1fff338fvdw53sqgamddn1b2xgds473pv6y13gizdbqjv4i5p3
//! FileSize: 4029176
//! NarHash: sha256:1impfw8zdgisxkghq9a3q7cn7jb9zyzgxdydiamp8z2nlyyl0h5h
//! NarSize: 18735072
//! References: 0d71ygfwbmy1xjlbj1v027dfmy9cqavy-libffi-3.3 0dbbrvlw2rahvzi69bmpqy1z9mvzg62s-gdbm-1.19 0i6vphc3vnr8mg0gxjr61564hnp0s2md-gnugrep-3.6 0vkw1m51q34dr64z5i87dy99an4hfmyg-coreutils-8.32 64ylsrpd025kcyi608w3dqckzyz57mdc-libyaml-0.2.5 65ys3k6gn2s27apky0a0la7wryg3az9q-zlib-1.2.11 9m4hy7cy70w6v2rqjmhvd7ympqkj6yxk-ncurses-6.2 a4yw1svqqk4d8lhwinn9xp847zz9gfma-bash-4.4-p23 hbm0951q7xrl4qd0ccradp6bhjayfi4b-openssl-1.1.1k hjwjf3bj86gswmxva9k40nqx6jrb5qvl-readline-6.3p08 p4pclmv1gyja5kzc26npqpia1qqxrf0l-ruby-2.7.3 sbbifs2ykc05inws26203h0xwcadnf0l-glibc-2.32-46Deriver: bidkcs01mww363s4s7akdhbl6ws66b0z-ruby-2.7.3.drv
//! Sig: cache.nixos.org-1:GrGV/Ls10TzoOaCnrcAqmPbKXFLLSBDeGNh5EQGKyuGA4K1wv1LcRVb6/sU+NAPK8lDiam8XcdJzUngmdhfTBQ==
//! ```
//!
//! Consult the following files for the Nix implementation:
//!
//! - `src/libstore/nar-info.cc`
//! - `src/libstore/path-info.hh`
//!
//! They provide valuable information on what are the required
//! fields.
//!
//! ## Fingerprint
//!
//! The fingerprint format is described in `perl/lib/Nix/Manifest.pm` (`sub
//! fingerprintAuth`). Each fingerprint contains the full store path, the
//! NAR hash, the NAR size, as well as a list of references (full store
//! paths). The format is as follows:
//!
//! ```text
//! 1;{storePath};{narHash};{narSize};{commaDelimitedReferences}
//! ```

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::string::ToString;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::de;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::error::{ErrorKind, ServerError, ServerResult};
use crate::nix_manifest::{self, SpaceDelimitedList};
use attic::hash::Hash;
use attic::mime;
use attic::signing::NixKeypair;

#[cfg(test)]
mod tests;

/// NAR information.
#[serde_as]
#[derive(Serialize, Deserialize)]
pub struct NarInfo {
    /// The full store path being cached, including the store directory.
    ///
    /// Part of the fingerprint.
    ///
    /// Example: `/nix/store/p4pclmv1gyja5kzc26npqpia1qqxrf0l-ruby-2.7.3`.
    #[serde(rename = "StorePath")]
    pub store_path: PathBuf,

    /// The URL to fetch the object.
    ///
    /// This can either be relative to the base cache URL (`cacheUri`),
    /// or be an full, absolute URL.
    ///
    /// Example: `nar/1w1fff338fvdw53sqgamddn1b2xgds473pv6y13gizdbqjv4i5p3.nar.xz`
    /// Example: `https://cache.nixos.org/nar/1w1fff338fvdw53sqgamddn1b2xgds473pv6y13gizdbqjv4i5p3.nar.xz`
    ///
    /// Nix implementation: <https://github.com/NixOS/nix/blob/af553b20902b8b8efbccab5f880879b09e95eb32/src/libstore/http-binary-cache-store.cc#L138-L145>
    #[serde(rename = "URL")]
    pub url: String,

    /// Compression in use.
    #[serde(rename = "Compression")]
    pub compression: Compression,

    /// The hash of the compressed file.
    ///
    /// We don't know the file hash if it's chunked.
    #[serde(rename = "FileHash")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_hash: Option<Hash>,

    /// The size of the compressed file.
    ///
    /// We may not know the file size if it's chunked.
    #[serde(rename = "FileSize")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_size: Option<usize>,

    /// The hash of the NAR archive.
    ///
    /// Part of the fingerprint.
    #[serde(rename = "NarHash")]
    pub nar_hash: Hash,

    /// The size of the NAR archive.
    ///
    /// Part of the fingerprint.
    #[serde(rename = "NarSize")]
    pub nar_size: usize,

    /// Other store paths this object directly refereces.
    ///
    /// This only includes the base paths, not the store directory itself.
    ///
    /// Part of the fingerprint.
    ///
    /// Example element: `j5p0j1w27aqdzncpw73k95byvhh5prw2-glibc-2.33-47`
    #[serde(rename = "References")]
    #[serde_as(as = "SpaceDelimitedList")]
    pub references: Vec<String>,

    /// The system this derivation is built for.
    #[serde(rename = "System")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,

    /// The derivation that produced this object.
    #[serde(rename = "Deriver")]
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_deriver")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deriver: Option<String>,

    /// The signature of the object.
    ///
    /// The `Sig` field can be duplicated to include multiple
    /// signatures, but we only support one for now.
    #[serde(rename = "Sig")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,

    /// The content address of the object.
    #[serde(rename = "CA")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca: Option<String>,
}

/// NAR compression type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Compression {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "xz")]
    Xz,
    #[serde(rename = "bzip2")]
    Bzip2,
    #[serde(rename = "br")]
    Brotli,
    #[serde(rename = "zstd")]
    Zstd,
}

impl NarInfo {
    /// Parses a narinfo from a string.
    pub fn from_str(manifest: &str) -> ServerResult<Self> {
        nix_manifest::from_str(manifest)
    }

    /// Returns the serialized representation of the narinfo.
    pub fn to_string(&self) -> ServerResult<String> {
        nix_manifest::to_string(self)
    }

    /// Returns the signature of this object, if it exists.
    pub fn signature(&self) -> Option<&String> {
        self.signature.as_ref()
    }

    /// Returns the store directory of this object.
    pub fn store_dir(&self) -> &Path {
        // FIXME: Validate store_path
        self.store_path.parent().unwrap()
    }

    /// Signs the narinfo and adds the signature to the narinfo.
    pub fn sign(&mut self, keypair: &NixKeypair) {
        let signature = self.sign_readonly(keypair);
        self.signature = Some(signature);
    }

    /// Returns the fingerprint of the object.
    pub fn fingerprint(&self) -> Vec<u8> {
        let store_dir = self.store_dir();
        let mut fingerprint = b"1;".to_vec();

        // 1;{storePath};{narHash};{narSize};{commaDelimitedReferences}

        // storePath
        fingerprint.extend(self.store_path.as_os_str().as_bytes());
        fingerprint.extend(b";");

        // narHash
        fingerprint.extend(self.nar_hash.to_typed_base32().as_bytes());
        fingerprint.extend(b";");

        // narSize
        let mut buf = itoa::Buffer::new();
        let nar_size = buf.format(self.nar_size);
        fingerprint.extend(nar_size.as_bytes());
        fingerprint.extend(b";");

        // commaDelimitedReferences
        let mut iter = self.references.iter().peekable();
        while let Some(reference) = iter.next() {
            fingerprint.extend(store_dir.as_os_str().as_bytes());
            fingerprint.extend(b"/");
            fingerprint.extend(reference.as_bytes());

            if iter.peek().is_some() {
                fingerprint.extend(b",");
            }
        }

        fingerprint
    }

    /// Signs the narinfo with a keypair, returning the signature.
    fn sign_readonly(&self, keypair: &NixKeypair) -> String {
        let fingerprint = self.fingerprint();
        keypair.sign(&fingerprint)
    }
}

impl IntoResponse for NarInfo {
    fn into_response(self) -> Response {
        match nix_manifest::to_string(&self) {
            Ok(body) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", mime::NARINFO)
                .body(body)
                .unwrap()
                .into_response(),
            Err(e) => e.into_response(),
        }
    }
}

impl Compression {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Xz => "xz",
            Self::Bzip2 => "bzip2",
            Self::Brotli => "br",
            Self::Zstd => "zstd",
        }
    }
}

impl FromStr for Compression {
    type Err = ServerError;

    fn from_str(s: &str) -> ServerResult<Self> {
        match s {
            "none" => Ok(Self::None),
            "xz" => Ok(Self::Xz),
            "bzip2" => Ok(Self::Bzip2),
            "br" => Ok(Self::Brotli),
            "zstd" => Ok(Self::Zstd),
            _ => Err(ErrorKind::InvalidCompressionType {
                name: s.to_string(),
            }
            .into()),
        }
    }
}

impl ToString for Compression {
    fn to_string(&self) -> String {
        String::from(self.as_str())
    }
}

pub fn deserialize_deriver<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    match s.as_str() {
        "unknown-deriver" => Ok(None),
        _ => Ok(Some(s)),
    }
}
