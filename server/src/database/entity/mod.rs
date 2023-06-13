//! Database entities.
//!
//! We use SeaORM and target PostgreSQL (production) and SQLite (development).

pub mod cache;
pub mod chunk;
pub mod chunkref;
pub mod nar;
pub mod object;

use sea_orm::entity::Value;
use sea_orm::sea_query::{ArrayType, ColumnType, ValueType, ValueTypeErr};
use sea_orm::{DbErr, QueryResult, TryGetError, TryGetable};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

// A more generic version of https://github.com/SeaQL/sea-orm/pull/783

/// A value that is stored in the database as JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Json<T>(pub T);

impl<T: Serialize + DeserializeOwned> From<Json<T>> for Value {
    fn from(value: Json<T>) -> Self {
        let opt = serde_json::to_string(&value).ok().map(Box::new);

        Value::String(opt)
    }
}

impl<T: Serialize + DeserializeOwned> TryGetable for Json<T> {
    fn try_get_by<I: sea_orm::ColIdx>(res: &QueryResult, idx: I) -> Result<Self, TryGetError> {
        let json_str: String = res.try_get_by(idx).map_err(TryGetError::DbErr)?;

        serde_json::from_str(&json_str).map_err(|e| TryGetError::DbErr(DbErr::Json(e.to_string())))
    }
}

impl<T: Serialize + DeserializeOwned> ValueType for Json<T> {
    fn try_from(v: Value) -> Result<Self, ValueTypeErr> {
        match v {
            Value::String(Some(x)) => Ok(Json(serde_json::from_str(&x).map_err(|_| ValueTypeErr)?)),
            _ => Err(ValueTypeErr),
        }
    }

    fn type_name() -> String {
        stringify!(Json<T>).to_owned()
    }

    fn column_type() -> ColumnType {
        ColumnType::String(None)
    }

    fn array_type() -> ArrayType {
        ArrayType::String
    }
}
