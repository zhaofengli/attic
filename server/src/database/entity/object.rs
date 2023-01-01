//! An object in a local cache.
//!
//! It's backed by a NAR in the global cache.

use std::path::PathBuf;
use std::str::FromStr;

use sea_orm::entity::prelude::*;

use super::nar::NarModel;
use super::Json;
use crate::error::{ServerError, ServerResult};
use crate::narinfo::{Compression, NarInfo};
use attic::hash::Hash;

pub type ObjectModel = Model;

/// An object in a binary cache.
#[derive(Debug, Clone, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "object")]
pub struct Model {
    /// Unique numeric ID of the object.
    #[sea_orm(primary_key)]
    pub id: i64,

    /// ID of the binary cache the object belongs to.
    #[sea_orm(indexed)]
    pub cache_id: i64,

    /// ID of the NAR this object points to.
    pub nar_id: i64,

    /// The hash portion of the store path.
    #[sea_orm(column_type = "String(Some(32))", indexed)]
    pub store_path_hash: String,

    /// The full store path being cached, including the store directory.
    pub store_path: String,

    /// Other store paths this object directly refereces.
    pub references: Json<Vec<String>>,

    /// The system this derivation is built for.
    pub system: Option<String>,

    /// The derivation that produced this object.
    pub deriver: Option<String>,

    /// Client-supplied signatures of this object.
    pub sigs: Json<Vec<String>>,

    /// The content address of this object.
    ///
    /// Technically this might belong to the NAR table since it's
    /// an inherent property of the content, but there are multiple
    /// formats for the CA and the feature isn't stable. So now we
    /// simply treat it as a untrusted string.
    pub ca: Option<String>,

    /// Timestamp when the object is created.
    pub created_at: ChronoDateTimeUtc,

    /// Timestamp when the object is last accessed.
    pub last_accessed_at: Option<ChronoDateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::cache::Entity",
        from = "Column::CacheId",
        to = "super::cache::Column::Id"
    )]
    Cache,

    #[sea_orm(
        belongs_to = "super::nar::Entity",
        from = "Column::NarId",
        to = "super::nar::Column::Id"
    )]
    Nar,
}

impl Model {
    /// Converts this object to a NarInfo.
    pub fn to_nar_info(&self, nar: &NarModel) -> ServerResult<NarInfo> {
        // FIXME: Return Err if file_hash and file_size don't exist
        let file_size = nar
            .file_size
            .unwrap()
            .try_into()
            .map_err(ServerError::database_error)?;
        let nar_size = nar
            .nar_size
            .try_into()
            .map_err(ServerError::database_error)?;

        Ok(NarInfo {
            store_path: PathBuf::from(self.store_path.to_owned()),
            url: format!("nar/{}.nar", self.store_path_hash.as_str()),

            compression: Compression::from_str(&nar.compression)?,
            file_hash: Hash::from_typed(nar.file_hash.as_ref().unwrap())?,
            file_size,
            nar_hash: Hash::from_typed(&nar.nar_hash)?,
            nar_size,
            system: self.system.to_owned(),
            references: self.references.0.to_owned(),
            deriver: self.deriver.to_owned(),
            signature: None,
            ca: self.ca.to_owned(),
        })
    }
}

impl Related<super::cache::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Cache.def()
    }
}

impl Related<super::nar::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Nar.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
