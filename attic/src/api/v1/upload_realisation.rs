use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A realisation document, as Nix writes/reads it for a ca-derivation output.
///
/// This mirrors the on-disk `.doi` shape Nix produces (verified against
/// Nix 2.34): `dependentRealisations` and `signatures` are round-tripped
/// verbatim but not interpreted by Attic — CA outputs are
/// self-authenticating, so an empty `signatures` array is normal and
/// doesn't mean the realisation is untrusted.
///
/// `unknown fields` are intentionally allowed (no `deny_unknown_fields`)
/// so a newer Nix that adds fields to this document doesn't fail to
/// upload through an older Attic server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Realisation {
    /// The resolved derivation output id, e.g. `sha256:<hex>!out`.
    pub id: String,

    /// The store path this output resolved to, as a **basename** (no
    /// `/nix/store/` prefix).
    #[serde(rename = "outPath")]
    pub out_path: String,

    /// Signatures over the realisation, as supplied by the client.
    #[serde(default)]
    pub signatures: Vec<String>,

    /// Realisations of the derivation's dependencies that this output
    /// depends on, keyed by their own derivation output id.
    #[serde(default, rename = "dependentRealisations")]
    pub dependent_realisations: HashMap<String, String>,
}
