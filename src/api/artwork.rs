//! The artwork catalogue endpoint.
//!
//! Lists what a title offers so a caller can choose. Without it, manual
//! selection would mean guessing a path, and the "not offered" rejection would
//! be the only way to discover what is valid.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::posters;
use crate::state::AppState;
use crate::tmdb::api::{Catalogue, MediaKind};

/// Query parameters for a catalogue listing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Params {
    /// Preferred language, as an ISO 639-1 code.
    ///
    /// Affects ordering rather than membership: artwork in other languages is
    /// still listed, below artwork in this one. A caller browsing for a French
    /// logo can still see the English one exists.
    pub language: Option<String>,
}

/// Lists the artwork a title offers, best first.
///
/// The order is the service's own ranking, so the first entry in each list is
/// exactly what a request with `"poster": "auto"` would select. That makes the
/// default visible rather than something a caller has to infer.
///
/// # Errors
///
/// [`ApiError::TmdbNotFound`] if the identifier is not in the catalogue, plus
/// the upstream and credential variants.
pub async fn list(
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, u32)>,
    Query(params): Query<Params>,
) -> Result<Json<Catalogue>, ApiError> {
    let kind = match kind.as_str() {
        "movie" => MediaKind::Movie,
        "tv" => MediaKind::Tv,
        other => {
            return Err(ApiError::MalformedRequest(format!(
                "unknown media kind `{other}`, expected `movie` or `tv`"
            )))
        }
    };

    let language = params
        .language
        .as_deref()
        .unwrap_or(&state.config.default_language)
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .take(2)
        .flat_map(char::to_lowercase)
        .collect::<String>();

    Ok(Json(
        posters::fetch_catalogue(&state, kind, id, &language).await?,
    ))
}
