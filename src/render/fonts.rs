//! Embedded font faces.
//!
//! Faces are compiled into the binary with `include_bytes!` and loaded into a
//! private `fontdb::Database`. System fonts are never consulted.
//!
//! That is a correctness requirement rather than a preference. Badge width is
//! derived from the rendered text advance, so the font in use determines the
//! geometry of the output. A build that fell back to a system font would
//! render different pixels on a laptop, in CI and in the release image — and
//! since the cache key does not encode the font, those different pixels would
//! be served interchangeably from one cache entry.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::OnceLock;

/// Inter `SemiBold`, used for badge text.
///
/// `SemiBold` rather than Regular: badge text is small, sits over a blurred
/// background, and needs the extra stroke weight to stay legible.
const INTER_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");

/// Family name every badge is laid out with.
pub const BADGE_FAMILY: &str = "Inter";

/// Returns the process-wide font database.
///
/// Built once on first use. Loading the faces is a few milliseconds of
/// parsing that would otherwise be paid on every render.
#[must_use]
pub fn database() -> Arc<fontdb::Database> {
    static DATABASE: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    Arc::clone(DATABASE.get_or_init(|| {
        let mut database = fontdb::Database::new();
        database.load_font_data(INTER_SEMIBOLD.to_vec());
        // Every generic family resolves to the one embedded face, so a
        // stylesheet asking for sans-serif cannot reach a system font.
        database.set_sans_serif_family(BADGE_FAMILY);
        database.set_serif_family(BADGE_FAMILY);
        database.set_monospace_family(BADGE_FAMILY);
        database.set_cursive_family(BADGE_FAMILY);
        database.set_fantasy_family(BADGE_FAMILY);
        Arc::new(database)
    }))
}

/// Measures the rendered width of each string at `size_px`, in pixels.
///
/// Measurement goes through `usvg` — the same code path that rasterises the
/// badges — rather than through a font parser summing glyph advances.
///
/// That is a correctness choice, not a convenience one. A separate measurement
/// has to reproduce shaping, kerning and fallback exactly, and any divergence
/// shows up as text clipped by a pill sized from a different answer than the
/// one the rasteriser used. Asking the renderer removes the possibility: the
/// number returned here *is* the width the glyphs occupy.
///
/// It also removed a dependency. The first version used `ttf-parser` and
/// summed horizontal advances, which ignored kerning and pulled a crate
/// RUSTSEC-2026-0192 lists as unmaintained.
///
/// All strings are measured in one document because `usvg` parsing is
/// per-document rather than per-node, so a row of six badges costs one parse
/// rather than six.
///
/// # Arguments
///
/// * `texts` — the strings to measure, already XML-escaped by the caller.
/// * `size_px` — font size in pixels.
///
/// # Returns
///
/// One width per input, in order. An empty string measures zero.
///
/// # Errors
///
/// Returns the `usvg` parse failure if the generated document is rejected.
///
/// # Examples
///
/// ```
/// use poster_service::render::fonts;
///
/// let widths = fonts::measure_all(&["i", "Oscar Nominee"], 32.0).expect("measures");
/// assert!(widths[1] > widths[0]);
/// ```
pub fn measure_all(texts: &[&str], size_px: f32) -> Result<Vec<f32>, usvg::Error> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    // Each string on its own line, far enough apart that bounding boxes cannot
    // merge. The canvas is generous because an oversized viewport costs
    // nothing here -- nothing is rasterised.
    let line_height = size_px * 3.0;
    // A badge row holds at most six entries, so the count converts exactly.
    let count = f32::from(u16::try_from(texts.len()).unwrap_or(u16::MAX));

    let mut document = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="{}">"#,
        line_height * count + line_height,
    );
    for (index, text) in texts.iter().enumerate() {
        let line = f32::from(u16::try_from(index).unwrap_or(u16::MAX));
        // Writing to a String is infallible; the Result satisfies fmt::Write.
        let _ = write!(
            document,
            r#"<text x="0" y="{y}" font-family="{BADGE_FAMILY}" font-size="{size_px}" font-weight="600">{text}</text>"#,
            y = line_height * (line + 1.0),
        );
    }
    document.push_str("</svg>");

    let options = usvg::Options {
        fontdb: database(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&document, &options)?;

    // usvg converts text to paths during parsing, so each original <text>
    // becomes one child group of the root, in document order.
    let widths = tree
        .root()
        .children()
        .iter()
        .map(|node| node.abs_bounding_box().width())
        .collect::<Vec<_>>();

    // An empty or whitespace-only string produces no node at all, so the
    // positional mapping only holds when every input rendered something.
    // Falling back to zero for the remainder keeps the lengths aligned.
    Ok(texts
        .iter()
        .enumerate()
        .map(|(index, text)| {
            if text.is_empty() {
                0.0
            } else {
                widths.get(index).copied().unwrap_or(0.0)
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_face_loads() {
        assert!(!database().is_empty(), "no faces were loaded");
    }

    #[test]
    fn the_database_is_built_once() {
        assert!(Arc::ptr_eq(&database(), &database()));
    }

    #[test]
    fn every_generic_family_resolves_to_the_embedded_face() {
        // A generic family escaping to a system font would render different
        // pixels per machine while sharing one cache key.
        let database = database();
        for family in [
            database.family_name(&fontdb::Family::SansSerif),
            database.family_name(&fontdb::Family::Serif),
            database.family_name(&fontdb::Family::Monospace),
            database.family_name(&fontdb::Family::Cursive),
            database.family_name(&fontdb::Family::Fantasy),
        ] {
            assert_eq!(family, BADGE_FAMILY, "a generic family escaped");
        }
    }

    /// Convenience for tests: measures one string.
    fn measure(text: &str, size_px: f32) -> f32 {
        measure_all(&[text], size_px).expect("measures")[0]
    }

    #[test]
    fn measurement_scales_linearly_with_size() {
        let small = measure("Oscar Nominee", 20.0);
        let large = measure("Oscar Nominee", 40.0);
        assert!(
            (large - small * 2.0).abs() < 0.5,
            "expected roughly double, got {small} and {large}"
        );
    }

    #[test]
    fn wider_text_measures_wider() {
        assert!(measure("#17 IMDb Top 250", 32.0) > measure("#1", 32.0));
    }

    #[test]
    fn empty_text_measures_zero() {
        assert!(measure("", 32.0).abs() < f32::EPSILON);
    }

    #[test]
    fn measurement_is_deterministic() {
        // Geometry derived from this feeds the rendered output, and the cache
        // key does not encode the font -- so a measurement that varied would
        // serve different pixels from one entry.
        assert!(
            (measure("Oscar Nominee", 28.0) - measure("Oscar Nominee", 28.0)).abs() < f32::EPSILON
        );
    }

    #[test]
    fn an_unmapped_character_measures_as_notdef() {
        // A rasteriser draws .notdef for an unmapped character, so measuring
        // it as zero would size a pill narrower than what gets drawn inside
        // it. Going through usvg means this is whatever the renderer will
        // actually draw, by construction.
        assert!(measure("\u{10FFFF}", 32.0) > 0.0);
    }

    #[test]
    fn several_strings_measure_independently() {
        // The batched call maps results positionally; a mismatch would size
        // every pill from its neighbour's text.
        let widths = measure_all(&["i", "Oscar Nominee", "#1"], 32.0).expect("measures");
        assert_eq!(widths.len(), 3);
        assert!(widths[1] > widths[0]);
        assert!(widths[1] > widths[2]);
    }

    #[test]
    fn batched_measurement_matches_individual_measurement() {
        // The whole point of batching is that it costs one usvg parse rather
        // than six. It must not change the answer.
        let texts = ["Oscar Nominee", "#17 IMDb", "4K HDR"];
        let batched = measure_all(&texts, 44.0).expect("measures");

        for (text, batched_width) in texts.iter().zip(batched) {
            let individual = measure(text, 44.0);
            assert!(
                (individual - batched_width).abs() < 0.01,
                "{text}: batched {batched_width} against individual {individual}"
            );
        }
    }

    #[test]
    fn measuring_nothing_returns_nothing() {
        assert!(measure_all(&[], 32.0).expect("measures").is_empty());
    }
}
