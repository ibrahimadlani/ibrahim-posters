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
        // `null` and `xx` are how TMDB marks artwork with no language: "No
        // Language" and "Not Specified". For posters those are the textless
        // versions -- no title, no credits block -- which is exactly what a
        // background wants when a logo is going to be placed over it.
        &[("include_image_language", &format!("{language},en,null,xx"))],
    )
    .await?;

    let mut posters = rank_posters(&images.posters, language);

    // The editorially primary poster is a last resort, not a default. It is
    // almost always the titled one -- Inception's is tagged `en` -- so adding
    // it to a list of textless posters would undo the filter above. It is used
    // only when the title offers nothing else, which happens when every one of
    // its posters carries a language the include filter excluded.
    if posters.is_empty() {
        if let Some(primary) = detail
            .poster_path
            .as_deref()
            .and_then(|path| PosterPath::parse(path).ok())
        {
            posters.push(ArtworkOption {
                path: primary,
                language: None,
                vote_average: 0.0,
                vote_count: 0,
                width: 0,
                height: 0,
            });
        }
    }

    Ok(Catalogue {
        kind,
        id,
        posters,
        logos: rank_logos(&images.logos, language),
    })
}

/// Reports whether an entry carries no language.
///
/// TMDB expresses this two ways: a null `iso_639_1`, shown as "No Language",
/// and the code `xx`, shown as "Not Specified". They mean the same thing for
/// our purposes and are treated alike.
fn is_textless(language: Option<&str>) -> bool {
    matches!(language, None | Some("xx"))
}

/// Returns the textless posters a title offers, best first.
///
/// **Filtered, not merely ordered.** A poster here is a background that a
/// title logo gets composited onto, so one that already carries a title
/// treatment is not a worse choice — it is the wrong kind of thing. Offering
/// it invites a poster with its title printed twice.
///
/// TMDB marks textless artwork two ways: a null `iso_639_1` ("No Language")
/// and the code `xx` ("Not Specified").
///
/// # The fallback, and why it exists
///
/// A title with *no* textless poster falls back to everything it has, ranked
/// by the requested language. That is not hedging: measured across twenty
/// titles, every popular one offered between 4 and 32 textless posters, but
/// four of ten obscure ones offered none at all — Ariel, Shadows in Paradise,
/// Four Rooms and Local Hero among them. Without the fallback those titles
/// would return `no_artwork_available` and could not be rendered at all,
/// which is a worse outcome than a poster whose title shows through.
///
/// A caller can tell which happened without a flag: every option carries its
/// `language`, so a list where none is null or `xx` is a list that fell back.
fn rank_posters(entries: &[ImageEntry], language: &str) -> Vec<ArtworkOption> {
    let textless: Vec<ArtworkOption> = rank_by(entries, |option| {
        u8::from(!is_textless(option.language.as_deref()))
    })
    .into_iter()
    .filter(|option| is_textless(option.language.as_deref()))
    .collect();

    if !textless.is_empty() {
        return textless;
    }

    rank_by(entries, |option| {
        u8::from(option.language.as_deref() != Some(language))
    })
}

/// Orders logos best first, preferring the requested language.
///
/// The reverse of posters, and for the reverse reason: a logo *is* the title,
/// so its language is the whole point. A language-neutral logo is the next
/// best thing, since most wordmarks carry no language tag at all.
fn rank_logos(entries: &[ImageEntry], language: &str) -> Vec<ArtworkOption> {
    rank_by(entries, |option| {
        if option.language.as_deref() == Some(language) {
            0
        } else if is_textless(option.language.as_deref()) {
            1
        } else {
            2
        }
    })
}

/// Orders entries by a language preference, dropping any this service cannot
/// render.
///
/// Unrenderable entries are skipped rather than failing the lookup, and the
/// reason is not only defensive: TMDB serves many logos as SVG, and
/// rasterising vector artwork fetched from a third party through `resvg` is a
/// materially larger attack surface than decoding a bitmap. A title whose only
/// logo is an SVG is offered no logo rather than an unsafe one.
///
/// Sorted rather than filtered, so a title with nothing in the preferred band
/// still offers something.
fn rank_by(entries: &[ImageEntry], band: impl Fn(&ArtworkOption) -> u8) -> Vec<ArtworkOption> {
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
        band(a)
            .cmp(&band(b))
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
    fn only_textless_posters_are_offered() {
        // Filtered, not merely ordered. A poster carrying its own title is not
        // a worse background for a logo, it is the wrong kind of thing.
        let entries = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.jpg", Some("en"), 9.0, 900),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.jpg", None, 1.0, 1),
            entry("/cccccccccccccccccccccccccccc.jpg", Some("fr"), 8.0, 400),
        ];
        let ranked = rank_posters(&entries, "en");

        assert_eq!(ranked.len(), 1, "a localised poster survived the filter");
        assert_eq!(ranked[0].language, None);
    }

    #[test]
    fn a_title_with_no_textless_poster_falls_back_to_what_it_has() {
        // Four of ten obscure titles measured against live TMDB offered no
        // textless poster at all -- Ariel, Shadows in Paradise, Four Rooms and
        // Local Hero. Without this they could not be rendered at all, which is
        // worse than a poster whose title shows through.
        let entries = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.jpg", Some("fr"), 9.0, 900),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.jpg", Some("en"), 1.0, 1),
        ];
        let ranked = rank_posters(&entries, "en");

        assert_eq!(ranked.len(), 2, "the fallback dropped artwork");
        assert_eq!(
            ranked[0].language.as_deref(),
            Some("en"),
            "the fallback ignored the requested language"
        );
    }

    #[test]
    fn the_fallback_is_visible_from_the_response_alone() {
        // No flag is needed: a caller sees every option's language, so a list
        // with nothing null or `xx` is a list that fell back.
        let only_localised = [entry(
            "/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.jpg",
            Some("en"),
            9.0,
            9,
        )];
        let has_textless = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.jpg", Some("en"), 9.0, 9),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.jpg", None, 1.0, 1),
        ];

        assert!(rank_posters(&only_localised, "en")
            .iter()
            .all(|option| !is_textless(option.language.as_deref())));
        assert!(rank_posters(&has_textless, "en")
            .iter()
            .all(|option| is_textless(option.language.as_deref())));
    }

    #[test]
    fn not_specified_counts_as_textless_for_posters() {
        // TMDB marks artwork with no language two ways: a null code shown as
        // "No Language", and `xx` shown as "Not Specified". Both are textless,
        // and both must survive the filter.
        let entries = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.jpg", Some("en"), 9.0, 900),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.jpg", Some("xx"), 1.0, 1),
            entry("/cccccccccccccccccccccccccccc.jpg", None, 0.5, 1),
        ];
        let ranked = rank_posters(&entries, "en");

        assert_eq!(ranked.len(), 2, "only the two textless entries belong here");
        assert!(ranked
            .iter()
            .all(|option| is_textless(option.language.as_deref())));
    }

    #[test]
    fn posters_and_logos_rank_in_opposite_directions() {
        // Stated as one assertion because it is the invariant that is easy to
        // break by "tidying" the two functions into one.
        let entries = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png", Some("en"), 5.0, 10),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.png", None, 5.0, 10),
        ];

        assert_eq!(rank_posters(&entries, "en")[0].language, None);
        assert_eq!(
            rank_logos(&entries, "en")[0].language.as_deref(),
            Some("en")
        );
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
        let ranked = rank_logos(&entries, "fr");
        assert_eq!(ranked[0].language.as_deref(), Some("fr"));
    }

    #[test]
    fn language_neutral_artwork_outranks_the_wrong_language() {
        let entries = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png", Some("de"), 9.0, 500),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.png", None, 1.0, 1),
        ];
        let ranked = rank_logos(&entries, "fr");
        assert_eq!(ranked[0].language, None, "a neutral logo should win");
    }

    #[test]
    fn rating_breaks_a_tie_within_a_language() {
        let entries = [
            entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png", Some("en"), 3.0, 500),
            entry("/bbbbbbbbbbbbbbbbbbbbbbbbbbbb.png", Some("en"), 8.0, 10),
        ];
        let ranked = rank_logos(&entries, "en");
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
        let ranked = rank_logos(&entries, "en");
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
        let ranked = rank_logos(&entries, "en");

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
        assert!(rank_logos(&entries, "en").is_empty());
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
        assert_eq!(rank_logos(&entries, "en"), rank_logos(&entries, "en"));
        assert_eq!(rank_posters(&entries, "en"), rank_posters(&entries, "en"));
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
            posters: rank_posters(
                &[entry("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.jpg", None, 5.0, 1)],
                "en",
            ),
            logos: rank_logos(
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
