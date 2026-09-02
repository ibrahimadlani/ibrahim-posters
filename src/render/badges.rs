//! The badge row: text measurement, pill geometry, SVG construction and
//! rasterisation.
//!
//! # Why one SVG for the whole row
//!
//! `usvg` parsing dominates the cost of this stage and is per *document*, not
//! per node. Building one document containing every pill and every label costs
//! one parse; building one per badge costs six.
//!
//! # Why SVG at all
//!
//! Text shaping is the hard part, and `resvg` already owns a correct
//! implementation of it. Emitting geometry and letting `resvg` shape the text
//! is considerably less code than driving a shaper directly, and it means the
//! output matches what a browser renders for the same markup — which is what
//! makes designing a badge in a browser meaningful.

use std::fmt::Write as _;

use tiny_skia::{Pixmap, Transform};

use crate::render::fonts;
use crate::render::palette::BadgePalette;
use crate::render::RenderError;
use crate::spec::{Badge, BadgeStyle};

/// Horizontal padding inside a pill, as a fraction of pill height.
const PADDING_RATIO: f32 = 0.45;
/// Gap between adjacent pills, as a fraction of pill height.
const GAP_RATIO: f32 = 0.30;
/// Font size of a text-sized pill, as a fraction of its height.
const FONT_RATIO: f32 = 0.46;

/// Font size of a fixed-width bar, as a fraction of its height.
///
/// Larger than a pill's. A pill is sized to its text, so its height follows
/// from the font; a bar's height is fixed and the text has to fill it, and at
/// the pill's ratio it floats in the middle of a bar looking undersized.
const BAR_FONT_RATIO: f32 = 0.60;
/// Corner radius of a text-sized pill, as a fraction of height. Half is a
/// full stadium.
const RADIUS_RATIO: f32 = 0.5;

/// Returns the font size for a badge of this height and layout mode.
///
/// Shared by measurement and by markup generation: sizing a pill from one
/// ratio and drawing it with another produces badges whose text overflows
/// them, and the two call sites are far enough apart to drift.
fn font_size(height: f32, fixed_width: bool) -> f32 {
    height
        * if fixed_width {
            BAR_FONT_RATIO
        } else {
            FONT_RATIO
        }
}

/// Corner radius of a fixed-width bar, as a fraction of height.
///
/// A bar is several times wider than it is tall, and a stadium radius on that
/// shape reads as a lozenge rather than as a rounded rectangle. The reference
/// posters use roughly this value.
const BAR_RADIUS_RATIO: f32 = 0.22;

/// Geometry of one laid-out badge.
#[derive(Debug, Clone, PartialEq)]
pub struct BadgeBox {
    /// Left edge, in pixels from the row's left edge.
    pub x: f32,
    /// Pill width in pixels, derived from the measured text advance.
    pub width: f32,
    /// The badge this geometry belongs to.
    pub badge: Badge,
}

/// A laid-out badge row.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// One entry per badge, left to right.
    pub boxes: Vec<BadgeBox>,
    /// Total width of the row in pixels, gaps included.
    pub total_width: f32,
    /// Pill height in pixels.
    pub height: f32,
    /// Whether each badge was given a fixed width rather than sized to its
    /// own text. Determines the corner radius.
    pub fixed_width: bool,
}

/// Lays out a badge row at the given pill height.
///
/// Each pill is sized from its own measured text, so the row is
/// variable-width by construction — a caller cannot request a pill narrower
/// than its contents, because a caller cannot request a width at all.
///
/// # Arguments
///
/// * `badges` — the badges, already normalised by
///   [`crate::spec::Badge::normalised`].
/// * `height` — pill height in pixels.
///
/// # Returns
///
/// The laid-out row. An empty input yields an empty layout of zero width.
///
/// # Errors
///
/// [`RenderError::Badges`] if the embedded font cannot be measured.
pub fn layout(
    badges: &[Badge],
    height: f32,
    fixed_width: Option<f32>,
) -> Result<Layout, RenderError> {
    let padding = height * PADDING_RATIO;
    let gap = height * GAP_RATIO;
    let font_size = font_size(height, fixed_width.is_some());

    // Escaped before measuring, so the string measured is the string that
    // ends up in the rendered document. Measuring the raw text and escaping
    // afterwards would size a pill from "&" and then draw "&amp;".
    let escaped: Vec<String> = badges.iter().map(|b| escape_xml(&b.text)).collect();
    let borrowed: Vec<&str> = escaped.iter().map(String::as_str).collect();

    let text_widths = fonts::measure_all(&borrowed, font_size)
        .map_err(|error| RenderError::Badges(format!("text measurement failed: {error}")))?;

    let mut boxes = Vec::with_capacity(badges.len());
    let mut cursor = 0.0_f32;

    for (badge, text_width) in badges.iter().zip(text_widths) {
        // A fixed width is a floor, never a ceiling: a badge is still allowed
        // to grow past it rather than have its text clipped, because a
        // clipped accolade is worse than an uneven row.
        let natural = text_width + padding * 2.0;
        let width = fixed_width.map_or(natural, |fixed| fixed.max(natural));

        boxes.push(BadgeBox {
            x: cursor,
            width,
            badge: badge.clone(),
        });
        cursor += width + gap;
    }

    // The trailing gap is not part of the row.
    let total_width = if boxes.is_empty() { 0.0 } else { cursor - gap };

    Ok(Layout {
        boxes,
        total_width,
        height,
        fixed_width: fixed_width.is_some(),
    })
}

/// Renders a laid-out row into a freshly allocated pixmap.
///
/// # Arguments
///
/// * `layout` — the row geometry.
///
/// # Returns
///
/// A pixmap exactly covering the row, or `None` if the row is empty.
///
/// # Errors
///
/// [`RenderError::Badges`] if the generated SVG cannot be parsed or
/// rasterised.
pub fn rasterise(
    layout: &Layout,
    derived: Option<BadgePalette>,
) -> Result<Option<Pixmap>, RenderError> {
    if layout.boxes.is_empty() {
        return Ok(None);
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let width = layout.total_width.ceil().max(1.0) as u32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let height = layout.height.ceil().max(1.0) as u32;

    let markup = build_svg(layout, derived);

    let options = usvg::Options {
        fontdb: fonts::database(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&markup, &options)
        .map_err(|error| RenderError::Badges(format!("svg parse failed: {error}")))?;

    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| RenderError::Badges("badge row dimensions are unusable".to_owned()))?;
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());

    Ok(Some(pixmap))
}

/// Builds the SVG document for a laid-out row.
///
/// Every piece of user-supplied text goes through [`escape_xml`]. The document
/// is assembled from a typed [`Layout`] rather than from request fields, so
/// the only caller-controlled value that reaches the markup is the badge text
/// itself.
fn build_svg(layout: &Layout, derived: Option<BadgePalette>) -> String {
    let height = layout.height;
    let radius = height
        * if layout.fixed_width {
            BAR_RADIUS_RATIO
        } else {
            RADIUS_RATIO
        };
    let font_size = font_size(height, layout.fixed_width);
    // Optical centring: text sits slightly above the geometric centre because
    // most badge text has no descenders, and centring on the full em box
    // leaves it looking low.
    let baseline = height * 0.5 + font_size * 0.35;

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{height}" viewBox="0 0 {w} {height}">"#,
        w = layout.total_width.max(1.0),
    );

    // Colours derived from the artwork override the style's fixed palette:
    // the point of deriving them is that they match this poster, which no
    // fixed palette can do.
    let derived = derived.map(|p| (p.fill.to_string(), p.ink.to_string()));

    for item in &layout.boxes {
        let (fill, text_fill, stroke) = match &derived {
            Some((fill, ink)) => (fill.as_str(), ink.as_str(), ""),
            None => palette(item.badge.style),
        };
        // Writing to a String is infallible; the Result exists only to
        // satisfy the fmt::Write signature.
        let _ = write!(
            svg,
            r#"<rect x="{x}" y="0" width="{width}" height="{height}" rx="{radius}" ry="{radius}" fill="{fill}"{stroke}/>"#,
            x = item.x,
            width = item.width,
        );
        let _ = write!(
            svg,
            r#"<text x="{x}" y="{baseline}" font-family="{family}" font-size="{font_size}" font-weight="600" fill="{text_fill}" text-anchor="middle">{text}</text>"#,
            x = item.x + item.width / 2.0,
            family = fonts::BADGE_FAMILY,
            text = escape_xml(&item.badge.text),
        );
    }

    svg.push_str("</svg>");
    svg
}

/// Returns the fill, text colour and stroke attribute for a style.
fn palette(style: BadgeStyle) -> (&'static str, &'static str, &'static str) {
    match style {
        BadgeStyle::Solid => ("#ffffff", "#111111", ""),
        BadgeStyle::Outline => (
            "none",
            "#ffffff",
            r##" stroke="#ffffff" stroke-width="2" stroke-opacity="0.85""##,
        ),
        BadgeStyle::Accent => ("#f5c518", "#111111", ""),
    }
}

/// Escapes the five XML metacharacters.
///
/// Badge text is caller-supplied. Without escaping, a badge reading
/// `</text><script>` would close the element and inject nodes into a document
/// this service generates — and while `resvg` executes no scripts, a text
/// value that can restructure the document can still draw arbitrary shapes
/// over the poster.
///
/// Control characters are already stripped upstream by
/// [`crate::spec::Badge::normalised`]; this handles the metacharacters that
/// survive it.
fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn badge(text: &str) -> Badge {
        Badge {
            text: text.to_owned(),
            style: BadgeStyle::Solid,
        }
    }

    #[test]
    fn an_empty_row_lays_out_to_nothing() {
        let laid_out = layout(&[], 44.0, None).expect("lays out");
        assert!(laid_out.boxes.is_empty());
        assert!(laid_out.total_width.abs() < f32::EPSILON);
        assert!(rasterise(&laid_out, None).expect("rasterises").is_none());
    }

    #[test]
    fn pill_width_follows_text_width() {
        // The property that makes the row variable-width. A caller cannot
        // request a width, so a pill can never be narrower than its contents.
        let laid_out =
            layout(&[badge("#1"), badge("#17 IMDb Top 250")], 44.0, None).expect("lays out");
        assert!(
            laid_out.boxes[1].width > laid_out.boxes[0].width,
            "longer text did not produce a wider pill"
        );
    }

    #[test]
    fn every_pill_is_wider_than_its_text() {
        let height = 44.0;
        let font_size = height * FONT_RATIO;
        let laid_out =
            layout(&[badge("Oscar Nominee"), badge("W")], height, None).expect("lays out");

        for item in &laid_out.boxes {
            let text = fonts::measure_all(&[&item.badge.text], font_size).expect("measures")[0];
            assert!(
                item.width > text,
                "pill {} is narrower than its text {text}",
                item.width
            );
        }
    }

    #[test]
    fn pills_do_not_overlap() {
        let laid_out = layout(
            &[badge("One"), badge("Two"), badge("Three, longer")],
            44.0,
            None,
        )
        .expect("lays out");

        for pair in laid_out.boxes.windows(2) {
            assert!(
                pair[1].x >= pair[0].x + pair[0].width,
                "pills overlap: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn the_row_width_excludes_the_trailing_gap() {
        let laid_out = layout(&[badge("A"), badge("B")], 40.0, None).expect("lays out");
        let last = laid_out.boxes.last().expect("non-empty");
        assert!(
            (laid_out.total_width - (last.x + last.width)).abs() < 0.001,
            "row width includes a trailing gap"
        );
    }

    #[test]
    fn layout_scales_with_height() {
        let small = layout(&[badge("Oscar Nominee")], 40.0, None).expect("lays out");
        let large = layout(&[badge("Oscar Nominee")], 80.0, None).expect("lays out");
        assert!(
            (large.total_width - small.total_width * 2.0).abs() < 0.01,
            "doubling the height did not double the row"
        );
    }

    #[test]
    fn xml_metacharacters_are_escaped() {
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml("</text>"), "&lt;/text&gt;");
        assert_eq!(escape_xml(r#"say "hi""#), "say &quot;hi&quot;");
        assert_eq!(escape_xml("it's"), "it&apos;s");
    }

    #[test]
    fn markup_injection_is_neutralised() {
        // Badge text is caller-supplied. resvg runs no scripts, but text that
        // can close an element can still draw arbitrary shapes over the
        // poster, so it must not survive into the document as markup.
        let hostile = badge(r#"</text><rect width="9999" height="9999" fill="red"/>"#);
        let laid_out = layout(&[hostile], 44.0, None).expect("lays out");
        let markup = build_svg(&laid_out, None);

        assert!(
            !markup.contains("<rect width=\"9999\""),
            "injected element survived into the document"
        );
        assert!(markup.contains("&lt;/text&gt;"), "text was not escaped");

        // And the document must still be valid: a broken escape would show up
        // as a parse failure rather than as an injection.
        assert!(rasterise(&laid_out, None).expect("rasterises").is_some());
    }

    #[test]
    fn a_row_rasterises_to_its_laid_out_size() {
        let laid_out =
            layout(&[badge("#17 IMDb"), badge("Oscar Nominee")], 44.0, None).expect("lays out");
        let pixmap = rasterise(&laid_out, None)
            .expect("rasterises")
            .expect("non-empty");

        assert_eq!(pixmap.height(), 44);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let expected = laid_out.total_width.ceil() as u32;
        assert_eq!(pixmap.width(), expected);
    }

    #[test]
    fn a_rasterised_row_actually_draws_something() {
        // Guards the failure where geometry is correct, the SVG parses, and
        // nothing is painted -- which a size assertion alone would pass.
        let laid_out = layout(&[badge("Oscar Nominee")], 44.0, None).expect("lays out");
        let pixmap = rasterise(&laid_out, None)
            .expect("rasterises")
            .expect("non-empty");

        let painted = pixmap.pixels().iter().filter(|p| p.alpha() > 0).count();
        let total = pixmap.pixels().len();
        assert!(
            painted > total / 2,
            "only {painted} of {total} pixels were painted"
        );
    }

    #[test]
    fn text_is_drawn_inside_the_pill() {
        // A solid pill is white with near-black text. If the text were
        // missing or drawn outside, no dark pixels would appear.
        let laid_out = layout(&[badge("Oscar Nominee")], 60.0, None).expect("lays out");
        let pixmap = rasterise(&laid_out, None)
            .expect("rasterises")
            .expect("non-empty");

        let dark = pixmap
            .pixels()
            .iter()
            .filter(|p| p.alpha() > 200 && p.red() < 80 && p.green() < 80 && p.blue() < 80)
            .count();
        assert!(dark > 50, "only {dark} dark pixels: text is missing");
    }

    #[test]
    fn each_style_rasterises() {
        for style in [BadgeStyle::Solid, BadgeStyle::Outline, BadgeStyle::Accent] {
            let laid_out = layout(
                &[Badge {
                    text: "Style".to_owned(),
                    style,
                }],
                44.0,
                None,
            )
            .expect("lays out");
            assert!(
                rasterise(&laid_out, None).expect("rasterises").is_some(),
                "{style:?} did not rasterise"
            );
        }
    }

    #[test]
    fn layout_is_deterministic() {
        // Geometry feeds rendered output, and the cache key does not encode
        // it, so a layout that varied would serve different pixels from one
        // cache entry.
        let badges = [badge("#17 IMDb"), badge("Oscar Nominee")];
        assert_eq!(
            layout(&badges, 44.0, None).expect("lays out"),
            layout(&badges, 44.0, None).expect("lays out")
        );
    }

    #[test]
    fn text_with_only_wide_characters_still_fits() {
        // Emoji and CJK measure far wider per character than Latin text; a
        // pill sized from a Latin assumption would clip them.
        let laid_out =
            layout(&[badge("\u{1F3AC}\u{1F3AC}\u{1F3AC}")], 44.0, None).expect("lays out");
        let font_size = 44.0 * FONT_RATIO;
        let text =
            fonts::measure_all(&["\u{1F3AC}\u{1F3AC}\u{1F3AC}"], font_size).expect("measures")[0];
        assert!(laid_out.boxes[0].width > text);
    }
}
