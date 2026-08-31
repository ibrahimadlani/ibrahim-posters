//! Fixtures shared across integration suites.
//!
//! Lives here rather than being duplicated per suite so that a change to the
//! catalogue shape is one edit, and so every suite agrees on what "a title
//! with artwork" means.

#![allow(dead_code)]

use poster_service::tmdb::api::{ArtworkOption, Catalogue, MediaKind};
use poster_service::tmdb::PosterPath;

/// The poster path every fixture composites on.
pub const POSTER: &str = "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg";
/// The logo path every fixture places.
pub const LOGO: &str = "/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png";

/// Builds one catalogue option.
pub fn option(path: &str) -> ArtworkOption {
    ArtworkOption {
        path: PosterPath::parse(path).expect("valid path"),
        language: Some("en".to_owned()),
        vote_average: 5.0,
        vote_count: 10,
        width: 2000,
        height: 3000,
    }
}

/// A catalogue offering one poster and one logo.
pub fn catalogue() -> Catalogue {
    Catalogue {
        kind: MediaKind::Movie,
        id: 27205,
        posters: vec![option(POSTER)],
        logos: vec![option(LOGO)],
    }
}

/// The JSON body a TMDB detail endpoint would return.
pub fn detail_body() -> serde_json::Value {
    serde_json::json!({ "id": 27205, "title": "Fixture", "poster_path": POSTER })
}

/// The JSON body a TMDB images endpoint would return.
pub fn images_body() -> serde_json::Value {
    serde_json::json!({
        "id": 27205,
        "posters": [
            { "file_path": POSTER, "iso_639_1": "en", "vote_average": 8.0,
              "vote_count": 100, "width": 2000, "height": 3000 }
        ],
        "logos": [
            { "file_path": LOGO, "iso_639_1": "en", "vote_average": 7.0,
              "vote_count": 50, "width": 1000, "height": 200 }
        ],
        "backdrops": []
    })
}
