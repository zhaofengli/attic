//! A realisation, mapping a resolved ca-derivation output to its store path.
//!
//! Nix's realisation mechanism is how content-addressed derivations get
//! substituted: the derivation output id (e.g. `sha256:<hex>!out`) doesn't
//! by itself say which store path it resolved to, so Nix asks the
//! substituter for `/{cache}/realisations/{id}.doi` and gets back a small
//! JSON document naming the output path (see api/binary_cache.rs and
//! api/v1/upload_realisation.rs). We store that document verbatim and key
//! it by cache + drv output id, mirroring how `object` rows are keyed by
//! cache + store path hash.

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::OnConflict;
use sea_orm::Insert;

pub type RealisationModel = Model;

pub trait InsertExt {
    fn on_conflict_do_update(self) -> Self;
}

/// A realisation in a binary cache.
#[derive(Debug, Clone, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "realisation")]
pub struct Model {
    /// Unique numeric ID of the realisation.
    #[sea_orm(primary_key)]
    pub id: i64,

    /// ID of the binary cache the realisation belongs to.
    #[sea_orm(indexed)]
    pub cache_id: i64,

    /// The resolved derivation output id, e.g. `sha256:<hex>!out`.
    pub drv_output_id: String,

    /// The raw `.doi` JSON document, stored verbatim as received from the
    /// client and served verbatim to substituting clients.
    #[sea_orm(column_type = "Text")]
    pub data: String,

    /// Timestamp when the realisation is created.
    pub created_at: ChronoDateTimeUtc,

    /// The uploader of the realisation.
    ///
    /// This is a "username." Currently, it's set to the `sub` claim in
    /// the client's JWT. Mirrors `object.created_by`.
    pub created_by: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::cache::Entity",
        from = "Column::CacheId",
        to = "super::cache::Column::Id"
    )]
    Cache,
}

impl InsertExt for Insert<ActiveModel> {
    fn on_conflict_do_update(self) -> Self {
        self.on_conflict(
            OnConflict::columns([Column::CacheId, Column::DrvOutputId])
                .update_columns([Column::Data, Column::CreatedAt, Column::CreatedBy])
                .to_owned(),
        )
    }
}

impl Related<super::cache::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Cache.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
