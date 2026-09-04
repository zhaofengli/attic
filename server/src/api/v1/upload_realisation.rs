//! Uploads a realisation for a resolved ca-derivation output.
//!
//! Realisations are how Nix substitutes content-addressed derivation
//! outputs: before a CA output can be pulled from a cache, the cache must
//! be able to answer "what store path did derivation output {id} resolve
//! to." This endpoint is the write side of that (see api/binary_cache.rs
//! for the read side, and attic issue #188 for why Attic didn't support
//! this until now).

use std::sync::LazyLock;

use axum::body::Bytes;
use axum::extract::{Extension, Path};
use chrono::Utc;
use regex::Regex;
use sea_orm::ActiveValue::Set;
use sea_orm::entity::prelude::*;
use tracing::instrument;

use crate::database::entity::realisation::{self, Entity as Realisation, InsertExt};
use crate::error::{ErrorKind, ServerError, ServerResult};
use crate::{RequestState, State};
use attic::api::v1::upload_realisation::Realisation as RealisationDoc;
use attic::cache::CacheName;

/// Maximum accepted size of a `.doi` body.
///
/// Realisations are tiny JSON documents (an id, an output path basename,
/// and some signatures); anything past this is not a realisation.
const MAX_REALISATION_SIZE: usize = 64 * 1024;

/// Matches a plausible Nix store path basename: a 32-character hash
/// followed by `-` and a name, e.g. `<hash>-hello-2.12`.
///
/// This is only meant to catch obviously-wrong `outPath` values before we
/// store them; it's not a full store path validator.
static STORE_PATH_BASENAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-z]{32}-").unwrap());

/// Uploads a realisation to a cache.
///
/// `PUT /_api/v1/upload-realisation/{cache}/{id}`
///
/// The body is the raw `.doi` JSON document. We store it verbatim (it's
/// what gets served back byte-for-byte from the GET route), but validate
/// enough of its shape first that a malformed upload fails loudly here
/// rather than producing a realisation the substituter side can't make
/// sense of later.
#[instrument(skip_all, fields(cache_name, drv_output_id))]
#[axum_macros::debug_handler]
pub(crate) async fn upload_realisation(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path((cache_name, drv_output_id)): Path<(CacheName, String)>,
    body: Bytes,
) -> ServerResult<()> {
    if body.len() > MAX_REALISATION_SIZE {
        return Err(ErrorKind::RequestError(anyhow::anyhow!(
            "Realisation document is too large"
        ))
        .into());
    }

    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_push()?;
            Ok(cache)
        })
        .await?;

    let doc: RealisationDoc =
        serde_json::from_slice(&body).map_err(ServerError::request_error)?;

    if doc.id != drv_output_id {
        return Err(ErrorKind::RequestError(anyhow::anyhow!(
            "Realisation id in body ({}) does not match the URL ({})",
            doc.id,
            drv_output_id
        ))
        .into());
    }

    if !STORE_PATH_BASENAME_RE.is_match(&doc.out_path) {
        return Err(
            ErrorKind::RequestError(anyhow::anyhow!("outPath is not a valid store path basename"))
                .into(),
        );
    }

    let username = req_state.auth.username().map(str::to_string);
    let data = std::str::from_utf8(&body)
        .map_err(ServerError::request_error)?
        .to_string();

    Realisation::insert(realisation::ActiveModel {
        cache_id: Set(cache.id),
        drv_output_id: Set(drv_output_id),
        data: Set(data),
        created_at: Set(Utc::now()),
        created_by: Set(username),
        ..Default::default()
    })
    .on_conflict_do_update()
    .exec(database)
    .await
    .map_err(ServerError::database_error)?;

    Ok(())
}
