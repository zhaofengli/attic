//! A binary cache.

use sea_orm::entity::prelude::*;

use super::Json;
use attic::error::AtticResult;
use attic::signing::NixKeypair;

pub type CacheModel = Model;

/// A binary cache.
#[derive(Debug, Clone, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "cache")]
pub struct Model {
    /// Unique numeric ID of the cache.
    #[sea_orm(primary_key)]
    pub id: i64,

    /// Unique name of the cache.
    #[sea_orm(column_type = "String(StringLen::N(50))", unique, indexed)]
    pub name: String,

    /// Signing keypair for the cache.
    pub keypair: String,

    /// Whether the cache is public or not.
    ///
    /// Anonymous clients are implicitly granted the "pull"
    /// permission to public caches.
    pub is_public: bool,

    /// The Nix store path this binary cache uses.
    pub store_dir: String,

    /// The priority of the binary cache.
    ///
    /// A lower number denotes a higher priority.
    /// <https://cache.nixos.org> has a priority of 40.
    pub priority: i32,

    /// A list of signing key names for upstream caches.
    pub upstream_cache_key_names: Json<Vec<String>>,

    /// Timestamp when the binary cache is created.
    pub created_at: ChronoDateTimeUtc,

    /// Timestamp when the binary cache is deleted.
    pub deleted_at: Option<ChronoDateTimeUtc>,

    /// The retention period of the cache, in seconds.
    pub retention_period: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::object::Entity")]
    Object,
}

impl Model {
    pub fn keypair(&self) -> AtticResult<NixKeypair> {
        NixKeypair::from_str(&self.keypair)
    }
}

impl Related<super::object::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Object.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
