//! The preset catalogue endpoint.

use axum::Json;
use serde::Serialize;

use crate::spec::preset::{self, Preset};

/// One catalogue entry, with its values fully resolved.
#[derive(Debug, Serialize)]
pub struct CatalogueEntry {
    /// Name to pass as `preset` in a request.
    pub name: &'static str,
    /// The values this preset resolves to before any override.
    #[serde(flatten)]
    pub values: Preset,
}

/// The catalogue response.
#[derive(Debug, Serialize)]
pub struct Catalogue {
    /// Every preset, ordered by name.
    pub presets: Vec<CatalogueEntry>,
}

/// Returns the preset catalogue.
///
/// Values are the *resolved* defaults rather than the raw file contents, so a
/// client can predict what a request will produce without reimplementing the
/// merge.
///
/// # Returns
///
/// The catalogue as JSON.
pub async fn list() -> Json<Catalogue> {
    Json(Catalogue {
        presets: preset::catalogue_entries()
            .into_iter()
            .map(|(name, values)| CatalogueEntry { name, values })
            .collect(),
    })
}
