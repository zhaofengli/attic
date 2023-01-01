/// The distributor of this Attic client.
///
/// Common values include `nixpkgs`, `attic` and `dev`.
pub const ATTIC_DISTRIBUTOR: &str = if let Some(distro) = option_env!("ATTIC_DISTRIBUTOR") {
    distro
} else {
    "unknown"
};
