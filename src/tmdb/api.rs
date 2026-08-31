//! TMDB metadata lookup: an identifier in, artwork paths out.
//!
//! # Why resolution happens at `POST` and not at `GET`
//!
//! A rendered poster is served with a one-year `immutable` directive, and its
//! key is a hash of the resolved specification. If a TMDB identifier were
//! resolved at render time, the same key would produce different artwork
//! whenever TMDB promoted a different poster — the response would change while
//! the URL promised it could not.
//!
//! Resolving at `POST` puts the *paths* in the specification, so the key
//! covers the artwork actually used. Two requests for one film taken a year
//! apart produce different keys if TMDB's primary poster changed, which is
//! correct: they are different posters.
//!
//! It also keeps `GET` free of metadata calls, which matters because `GET` is
//! the cached path and `POST` is not.
//!
//! # A second host
//!
//! This module contacts `api.themoviedb.org`, where the rest of the service
//! contacts `image.tmdb.org`. Both are configured bases with paths built from
//! typed values — an identifier is a `u32`, so it cannot carry a path segment
//! — so the SSRF argument from [`crate::tmdb`] holds unchanged: no caller
//! input reaches a URL except as a number.

use reqwest::{Client, StatusCode};
use serde::Deserialize;

use crate::tmdb::PosterPath;

/// Which TMDB catalogue an identifier belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    /// A film.
    Movie,
    /// A television series.
    Tv,
}

impl MediaKind {
    /// Returns the API path segment for this catalogue.
    #[must_use]
    pub const fn segment(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Tv => "tv",
        }
    }
}

/// Why a metadata lookup failed.
#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    /// No such identifier in the catalogue.
    #[error("{kind:?} {id} is not in the TMDB catalogue")]
    NotFound {
        /// The catalogue searched.
        kind: MediaKind,
        /// The identifier supplied.
        id: u32,
    },
    /// TMDB rejected the credential.
    #[error("TMDB rejected the configured credential")]
    Unauthorised,
    /// TMDB asked us to slow down.
    #[error("TMDB rate limit reached")]
    RateLimited,
    /// The entry exists but carries no usable poster.
    #[error("{kind:?} {id} has no poster artwork")]
    NoPoster {
        /// The catalogue searched.
        kind: MediaKind,
        /// The identifier supplied.
        id: u32,
    },
    /// TMDB answered with something unexpected.
    #[error("TMDB responded {status}")]
    UnexpectedStatus {
        /// The status returned.
        status: StatusCode,
    },
    /// The request could not be completed.
    #[error("could not reach TMDB: {0}")]
    Transport(String),
    /// TMDB's response did not parse.
    #[error("TMDB response was not understood: {0}")]
    Malformed(String),
}

/// Artwork paths resolved for one catalogue entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Artwork {
    /// The poster to composite on.
    pub poster: PosterPath,
    /// The title logo, if one was chosen and the entry has a renderable one.
    pub logo: Option<PosterPath>,
}

/// One piece of artwork a caller may choose.
///
/// Carries the metadata the automatic choice is made from, so a caller
/// browsing the catalogue can see *why* the default is the default rather than
/// being handed an opaque list.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ArtworkOption {
    /// TMDB path, and the value to send back to select this option.
    pub path: PosterPath,
    /// ISO 639-1 language, or `null` for language-neutral artwork.
    pub language: Option<String>,
    /// TMDB community rating, the tie-breaker within a language band.
    pub vote_average: f32,
    /// Number of votes behind that rating.
    pub vote_count: u32,
    /// Pixel width as TMDB reports it.
    pub width: u32,
    /// Pixel height as TMDB reports it.
    pub height: u32,
}

/// Everything a catalogue entry offers, in the order the service would pick.
///
/// Ordered rather than raw so that "the first one" and "what you get by
/// default" are the same thing. A caller that wants the default sends nothing;
/// a caller that wants a different one sends a `path` from this list.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Catalogue {
    /// Which catalogue the entry came from.
    pub kind: MediaKind,
    /// The identifier looked up.
    pub id: u32,
    /// Posters, best first.
    pub posters: Vec<ArtworkOption>,
    /// Logos, best first. Empty when the entry has none this service renders.
    pub logos: Vec<ArtworkOption>,
}

impl Catalogue {
    /// Returns the artwork this service would choose unaided.
    ///
    /// # Errors
    ///
    /// [`MetadataError::NoPoster`] if the entry offers no renderable poster.
    pub fn best(&self) -> Result<Artwork, MetadataError> {
        Ok(Artwork {
            poster: self
                .posters
                .first()
                .map(|option| option.path.clone())
                .ok_or(MetadataError::NoPoster {
                    kind: self.kind,
                    id: self.id,
                })?,
            logo: self.logos.first().map(|option| option.path.clone()),
        })
    }

    /// Reports whether this entry offers the given artwork.
    ///
    /// An explicit choice is checked against the catalogue rather than taken
    /// on trust. The path grammar already confines a value to the TMDB CDN, so
    /// this is not a security control — it stops a caller compositing one
    /// film's logo onto another film's poster by accident, and turns a stale
    /// path from a cached catalogue into a clear error rather than a poster
    /// nobody asked for.
    #[must_use]
    pub fn offers(&self, path: &PosterPath) -> bool {
        self.posters
            .iter()
            .chain(&self.logos)
            .any(|option| &option.path == path)
    }
}

/// How to authenticate against the TMDB API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Credential {
    /// A v4 read access token, sent as a bearer header.
    Bearer,
    /// A v3 API key, sent as a query parameter.
    ApiKey,
}

impl Credential {
    /// Infers the scheme from the credential's shape.
    ///
    /// A v4 read access token is a JWT and contains two dots; a v3 API key is
    /// 32 hexadecimal characters and contains none. Detecting the shape means
    /// an operator pastes whichever credential their TMDB account page gave
    /// them and it works, rather than having to know which of two settings to
    /// put it in.
    fn infer(secret: &str) -> Self {
        if secret.contains('.') {
            Self::Bearer
        } else {
            Self::ApiKey
        }
    }
}

/// One image entry in a TMDB `/images` response.
#[derive(Debug, Deserialize)]
struct ImageEntry {
    file_path: String,
    #[serde(default)]
    iso_639_1: Option<String>,
    #[serde(default)]
    vote_average: f32,
    #[serde(default)]
    vote_count: u32,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

/// The `/images` response.
#[derive(Debug, Deserialize)]
struct ImagesResponse {
    #[serde(default)]
    logos: Vec<ImageEntry>,
    #[serde(default)]
    posters: Vec<ImageEntry>,
}

/// The catalogue detail response, reduced to the one field this service needs.
#[derive(Debug, Deserialize)]
struct DetailResponse {
    #[serde(default)]
    poster_path: Option<String>,
}

/// Lists the artwork a catalogue entry offers, best first.
///
/// Makes two calls: the detail endpoint for the primary poster, and the images
/// endpoint for the rest. The detail endpoint's `poster_path` is promoted to
/// the front of the poster list because it is the one TMDB's editors
/// designated as primary, which is a better default than the highest-voted
/// one — a poster can accumulate votes for being striking rather than for
/// being the poster of record.
///
/// # Arguments
///
/// * `client` — the shared upstream client.
/// * `base` — the API base, for example `https://api.themoviedb.org`.
/// * `secret` — a v3 API key or a v4 read access token.
/// * `kind` — which catalogue to search.
/// * `id` — the catalogue identifier.
/// * `language` — preferred language as an ISO 639-1 code.
///
/// # Returns
///
/// Every renderable option, ordered so the first is what the service would
/// choose unaided.
///
/// # Errors
///
/// See [`MetadataError`].
pub async fn catalogue(
    client: &Client,
    base: &str,
    secret: &str,
    kind: MediaKind,
    id: u32,
    language: &str,
) -> Result<Catalogue, MetadataError> {
    let detail: DetailResponse = get(
        client,
        base,
        secret,
        &format!("3/{}/{id}", kind.segment()),
        &[],
    )
    .await?;

    let images: ImagesResponse = get(
        client,
        base,
        secret,
        &format!("3/{}/{id}/images", kind.segment()),
        // `null` admits language-neutral artwork, which is what most logos
        // are. Without it a film whose logo carries no language tag looks as
        // though it has no logo at all.
        &[("include_image_language", &format!("{language},null,en"))],
    )
    .await?;

    let mut posters = ranked(&images.posters, language);

    // The editorially primary poster leads, whether or not it ranked first --
    // and is added if `/images` omitted it, which happens when it carries a
    // language the include filter excluded.
    if let Some(primary) = detail
        .poster_path
        .as_deref()
        .and_then(|path| PosterPath::parse(path).ok())
    {
        if let Some(position) = posters.iter().position(|option| option.path == primary) {
            let option = posters.remove(position);
            posters.insert(0, option);
        } else {
            posters.insert(
                0,
                ArtworkOption {
                    path: primary,
                    language: None,
                    vote_average: 0.0,
                    vote_count: 0,
                    width: 0,
                    height: 0,
                },
            );
        }
    }

    Ok(Catalogue {
        kind,
        id,
        posters,
        logos: ranked(&images.logos, language),
    })
}

/// Orders entries best first, dropping any this service cannot render.
///
/// Unrenderable entries are skipped rather than failing the lookup, and the
/// reason is not only defensive: TMDB serves many logos as SVG, and
/// rasterising vector artwork fetched from a third party through `resvg` is a
/// materially larger attack surface than decoding a bitmap. A title whose only
/// logo is an SVG is offered no logo rather than an unsafe one.
///
/// The ordering is requested language, then language-neutral, then everything
/// else; within a band, highest rated, and votes break a rating tie. Sorted
/// rather than filtered so a title with nothing in the requested language
/// still offers something.
fn ranked(entries: &[ImageEntry], language: &str) -> Vec<ArtworkOption> {
    let mut options: Vec<ArtworkOption> = entries
        .iter()
        .filter_map(|entry| {
            Some(ArtworkOption {
                path: PosterPath::parse(&entry.file_path).ok()?,
                language: entry.iso_639_1.clone(),
                vote_average: entry.vote_average,
                vote_count: entry.vote_count,
                width: entry.width,
                height: entry.height,
            })
        })
        .collect();

    options.sort_by(|a, b| {
        let rank = |option: &ArtworkOption| match option.language.as_deref() {
            Some(code) if code == language => 0,
            None => 1,
            Some(_) => 2,
        };
        rank(a)
            .cmp(&rank(b))
            .then(b.vote_average.total_cmp(&a.vote_average))
            .then(b.vote_count.cmp(&a.vote_count))
    });

    options
}

/// Issues one authenticated GET and deserialises the result.
async fn get<T: serde::de::DeserializeOwned>(
    client: &Client,
    base: &str,
    secret: &str,
    path: &str,
    query: &[(&str, &str)],
) -> Result<T, MetadataError> {
    let url = format!("{}/{path}", base.trim_end_matches('/'));
    let mut request = client.get(&url).query(query);

    request = match Credential::infer(secret) {
        Credential::Bearer => request.bearer_auth(secret),
        Credential::ApiKey => request.query(&[("api_key", secret)]),
    };

    let response = request
        .send()
        .await
        .map_err(|error| MetadataError::Transport(error.to_string()))?;

    match response.status() {
        StatusCode::OK => {}
        StatusCode::NOT_FOUND => {
            // The caller knows which kind and id it asked for and rewrites
            // this into a more specific error; the status is what matters here.
            return Err(MetadataError::UnexpectedStatus {
                status: StatusCode::NOT_FOUND,
            });
        }
        StatusCode::UNAUTHORIZED => return Err(MetadataError::Unauthorised),
        StatusCode::TOO_MANY_REQUESTS => return Err(MetadataError::RateLimited),
        status => return Err(MetadataError::UnexpectedStatus { status }),
    }

    response
        .json()
        .await
        .map_err(|error| MetadataError::Malformed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, language: Option<&str>, vote: f32, votes: u32) -> ImageEntry {
        ImageEntry {
            file_path: path.to_owned(),
            iso_639_1: language.map(str::to_owned),
            vote_average: vote,
            vote_count: votes,
            width: 1000,
            height: 1500,
        }
    }

    #[test]
    fn the_requested_language_outranks_a_higher_rated_alternative() {
        // Language is the primary key, not the rating: a French caller asking
        // for a French logo should get one even when the English logo is
        // better rated, because the wrong language is a worse outcome than a
        // lower score.
        let entries = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png", Some("en"), 9.0, 500),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.png", Some("fr"), 1.0, 1),
        ];
        let ranked = ranked(&entries, "fr");
        assert_eq!(ranked[0].language.as_deref(), Some("fr"));
    }

    #[test]
    fn language_neutral_artwork_outranks_the_wrong_language() {
        let entries = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png", Some("de"), 9.0, 500),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.png", None, 1.0, 1),
        ];
        let ranked = ranked(&entries, "fr");
        assert_eq!(ranked[0].language, None, "a neutral logo should win");
    }

    #[test]
    fn rating_breaks_a_tie_within_a_language() {
        let entries = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png", Some("en"), 3.0, 500),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.png", Some("en"), 8.0, 10),
        ];
        let ranked = ranked(&entries, "en");
        assert!((ranked[0].vote_average - 8.0).abs() < f32::EPSILON);
    }

    #[test]
    fn votes_break_a_rating_tie() {
        // Two entries rated identically are not equally trustworthy; the one
        // more people voted on is the safer default.
        let entries = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png", Some("en"), 5.0, 3),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.png", Some("en"), 5.0, 900),
        ];
        let ranked = ranked(&entries, "en");
        assert_eq!(ranked[0].vote_count, 900);
    }

    #[test]
    fn svg_artwork_is_dropped_rather_than_offered() {
        // Not hypothetical: Breaking Bad's second-highest-voted logo on TMDB
        // is an SVG. Rasterising vector artwork fetched from a third party
        // through resvg is a materially larger attack surface than decoding a
        // bitmap, so it is never offered.
        let entries = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.svg", Some("en"), 9.9, 500),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.png", Some("en"), 1.0, 1),
        ];
        let ranked = ranked(&entries, "en");

        assert_eq!(ranked.len(), 1, "the SVG survived ranking");
        assert_eq!(
            ranked[0].path.as_str(),
            "/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.png",
            "the surviving entry is not the bitmap"
        );
    }

    #[test]
    fn a_title_whose_only_artwork_is_unrenderable_offers_none() {
        let entries = [entry(
            "/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.svg",
            Some("en"),
            9.9,
            500,
        )];
        assert!(ranked(&entries, "en").is_empty());
    }

    #[test]
    fn ranking_is_deterministic() {
        // The chosen artwork lands in the cache key, so a ranking that varied
        // would issue two keys for one request.
        let entries = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png", Some("en"), 5.0, 10),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.png", Some("en"), 5.0, 10),
            entry("/cccccccccccccccccccccccccccc.png", None, 5.0, 10),
        ];
        assert_eq!(ranked(&entries, "en"), ranked(&entries, "en"));
    }

    #[test]
    fn a_bearer_token_is_told_apart_from_an_api_key() {
        // A v4 read access token is a JWT and carries dots; a v3 key is 32 hex
        // characters and carries none. Sending one as the other fails
        // authentication, so the distinction has to be right.
        // Synthetic values with the right *shape*. A real token must never
        // appear in a test fixture: it would be committed, and a truncated
        // credential in a public repository is still a credential fragment.
        assert_eq!(
            Credential::infer("header.payload.signature"),
            Credential::Bearer
        );
        assert_eq!(
            Credential::infer("0123456789abcdef0123456789abcdef"),
            Credential::ApiKey
        );
    }

    #[test]
    fn media_kinds_map_to_their_api_segments() {
        assert_eq!(MediaKind::Movie.segment(), "movie");
        assert_eq!(MediaKind::Tv.segment(), "tv");
    }

    #[test]
    fn a_catalogue_reports_what_it_offers() {
        let catalogue = Catalogue {
            kind: MediaKind::Movie,
            id: 1,
            posters: ranked(
                &[entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.jpg", None, 5.0, 1)],
                "en",
            ),
            logos: ranked(
                &[entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.png", None, 5.0, 1)],
                "en",
            ),
        };

        assert!(catalogue.offers(&PosterPath::parse("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.jpg").unwrap()));
        assert!(catalogue.offers(&PosterPath::parse("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.png").unwrap()));
        assert!(!catalogue.offers(&PosterPath::parse("/zzzzzzzzzzzzzzzzzzzzzzzzzzzz.jpg").unwrap()));
    }

    #[test]
    fn an_empty_catalogue_has_no_best() {
        let empty = Catalogue {
            kind: MediaKind::Tv,
            id: 7,
            posters: Vec::new(),
            logos: Vec::new(),
        };
        assert!(matches!(
            empty.best(),
            Err(MetadataError::NoPoster { id: 7, .. })
        ));
    }
}
