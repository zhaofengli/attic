//! The Nix manifest format.
//!
//! Nix uses a simple format in binary cache manifests (`.narinfo`,
//! `/nix-cache-info`). It consists of a single, flat KV map with
//! colon (`:`) as the delimiter.
//!
//! It's not well-defined and the official implementation performs
//! serialization and deserialization by hand [1]. Here we implement
//! a deserializer and a serializer using the serde framework.
//!
//! An example of a `/nix-cache-info` file:
//!
//! ```text
//! StoreDir: /nix/store
//! WantMassQuery: 1
//! Priority: 40
//! ```
//!
//! [1] <https://github.com/NixOS/nix/blob/d581129ef9ef5d7d65e676f6a7bfe36c82f6ea6e/src/libstore/nar-info.cc#L28>

mod deserializer;
mod serializer;

#[cfg(test)]
mod tests;

use std::fmt::Display;
use std::result::Result as StdResult;

use displaydoc::Display;
use serde::{de, ser, Deserialize, Serialize};
use serde_with::{formats::SpaceSeparator, StringWithSeparator};

use crate::error::{ServerError, ServerResult};
use deserializer::Deserializer;
use serializer::Serializer;

type Result<T> = StdResult<T, Error>;

pub fn from_str<T>(s: &str) -> ServerResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    let mut deserializer = Deserializer::from_str(s);
    T::deserialize(&mut deserializer).map_err(ServerError::ManifestSerializationError)

    // FIXME: Reject extra output??
}

pub fn to_string<T>(value: &T) -> ServerResult<String>
where
    T: Serialize,
{
    let mut serializer = Serializer::new();
    value
        .serialize(&mut serializer)
        .map_err(ServerError::ManifestSerializationError)?;

    Ok(serializer.into_output())
}

/// An error during (de)serialization.
#[derive(Debug, Display)]
pub enum Error {
    /// Unexpected {0}.
    Unexpected(&'static str),

    /// Unexpected EOF.
    UnexpectedEof,

    /// Expected a colon.
    ExpectedColon,

    /// Expected a boolean.
    ExpectedBoolean,

    /// Expected an integer.
    ExpectedInteger,

    /// "{0}" values are unsupported.
    Unsupported(&'static str),

    /// Not possible to auto-determine the type.
    AnyUnsupported,

    /// None is unsupported. Add #[serde(skip_serializing_if = "Option::is_none")]
    NoneUnsupported,

    /// Nested maps are unsupported.
    NestedMapUnsupported,

    /// Floating point numbers are unsupported.
    FloatUnsupported,

    /// Custom error: {0}
    Custom(String),
}

/// Custom (de)serializer for a space-delimited list.
///
/// Example usage:
///
/// ```
/// use serde::Deserialize;
/// use serde_with::serde_as;
/// # use attic_server::nix_manifest::{self, SpaceDelimitedList};
///
/// #[serde_as]
/// #[derive(Debug, Deserialize)]
/// struct MyManifest {
///     #[serde_as(as = "SpaceDelimitedList")]
///     some_list: Vec<String>,
/// }
///
/// let s = "some_list: item-a item-b";
/// let parsed: MyManifest = nix_manifest::from_str(s).unwrap();
///
/// assert_eq!(vec![ "item-a", "item-b" ], parsed.some_list);
/// ```
pub type SpaceDelimitedList = StringWithSeparator<SpaceSeparator, String>;

impl std::error::Error for Error {}

impl de::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        let f = format!("{}", msg);
        Self::Custom(f)
    }
}

impl ser::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        let f = format!("{}", msg);
        Self::Custom(f)
    }
}
