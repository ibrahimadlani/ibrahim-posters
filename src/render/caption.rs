//! The genre and rating line beneath the logo.
//!
//! Kept apart from `badges` because the two solve different problems despite
//! both being text. A badge is a shape with text inside it, sized to its
//! content and placed against the top edge; a caption is a single centred line
//! with no shape at all, placed against the bottom. Sharing a code path would
//! mean one function branching on which of the two it was drawing.

use tiny_skia::{Pixmap, Transform};

use crate::render::fonts;
use crate::render::RenderError;
use crate::spec::Rgb;

/// Height of the rasterised strip, as a multiple of the text size.
///
/// The star reaches higher than the digits beside it and a descender can drop
/// below them, so the strip is taller than the nominal size to keep either
/// from being clipped at the edge of the pixmap.
const STRIP_RATIO: f32 = 1.6;

/// Fraction of the strip height that sits above the baseline.
const BASELINE_RATIO: f32 = 0.72;

/// Rasterises a caption line.
///
/// # Arguments
///
/// * `line` — the text, already assembled by [`crate::spec::Caption::line`].
/// * `size` — text size in pixels.
/// * `colour` — the text colour.
///
/// # Returns
///
/// A pixmap containing the centred line, or `None` if the line is empty.
///
/// # Errors
///
/// [`RenderError::Badges`] if the generated SVG cannot be measured, parsed or
/// rasterised.
pub fn rasterise(line: &str, size: f32, colour: Rgb) -> Result<Option<Pixmap>, RenderError> {
    if line.trim().is_empty() || size <= 0.0 {
        return Ok(None);
    }

    // Escaped before measuring, so the string measured is the string drawn.
    let escaped = escape_xml(line);
    let text_width = fonts::measure_all(&[escaped.as_str()], size)
        .map_err(|error| RenderError::Badges(format!("caption measurement failed: {error}")))?
        .first()
        .copied()
        .unwrap_or(0.0);

    let strip_height = size * STRIP_RATIO;
    // A little slack either side: measurement covers advances, and an italic
    // or a wide glyph can paint marginally past the last one.
    let strip_width = text_width + size;

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let width = strip_width.ceil().max(1.0) as u32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let height = strip_height.ceil().max(1.0) as u32;

    let markup = format!(
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"#,
            r#"<text x="{x}" y="{y}" font-family="{family}" font-size="{size}" "#,
            r#"font-weight="600" fill="{colour}" text-anchor="middle">{text}</text></svg>"#
        ),
        w = strip_width,
        h = strip_height,
        x = strip_width / 2.0,
        y = strip_height * BASELINE_RATIO,
        family = fonts::BADGE_FAMILY,
        // Named explicitly: `concat!` expands the format string from a macro,
        // which disables implicit capture of surrounding variables.
        size = size,
        colour = colour,
        text = escaped,
    );

    let options = usvg::Options {
        fontdb: fonts::database(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&markup, &options)
        .map_err(|error| RenderError::Badges(format!("caption svg parse failed: {error}")))?;

    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| RenderError::Badges("caption dimensions are unusable".to_owned()))?;
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());

    Ok(Some(pixmap))
}

/// Escapes the five XML metacharacters.
///
/// The genre label is caller-supplied, so it reaches the markup the same way
/// badge text does and needs the same treatment. See
/// [`crate::render::badges`] for why this matters even though `resvg` runs no
/// scripts.
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

    const INK: Rgb = Rgb::new(0xb8, 0xa8, 0xa0);

    /// Counts pixels carrying any paint.
    fn painted(pixmap: &Pixmap) -> usize {
        pixmap.pixels().iter().filter(|p| p.alpha() > 0).count()
    }

    #[test]
    fn an_empty_line_draws_nothing() {
        assert!(rasterise("", 88.0, INK).expect("rasterises").is_none());
        assert!(rasterise("   ", 88.0, INK).expect("rasterises").is_none());
    }

    #[test]
    fn a_zero_size_draws_nothing() {
        assert!(rasterise("Action", 0.0, INK).expect("rasterises").is_none());
    }

    #[test]
    fn the_star_and_separator_both_render() {
        // Both are non-ASCII and both come from the embedded face rather than
        // a system font, so a missing glyph would show as a blank rather than
        // as an error.
        let bare = rasterise("Action", 88.0, INK)
            .expect("rasterises")
            .expect("some");
        let full = rasterise("Action \u{b7} \u{2605} 6.5", 88.0, INK)
            .expect("rasterises")
            .expect("some");
        assert!(
            painted(&full) > painted(&bare),
            "the separator and star added no ink"
        );
    }

    #[test]
    fn the_line_scales_with_its_size() {
        let small = rasterise("Action \u{2605} 6.5", 40.0, INK)
            .expect("rasterises")
            .expect("some");
        let large = rasterise("Action \u{2605} 6.5", 80.0, INK)
            .expect("rasterises")
            .expect("some");
        assert!(large.width() > small.width());
        assert!(large.height() > small.height());
    }

    #[test]
    fn markup_in_a_genre_is_escaped_rather_than_parsed() {
        let hostile = "</text><rect width=\"999\" height=\"999\" fill=\"#ff0000\"/><text>";
        let pixmap = rasterise(hostile, 60.0, INK)
            .expect("rasterises")
            .expect("some");
        // An injected rect would paint a solid red block; escaped text does
        // not paint every pixel.
        let filled = painted(&pixmap);
        assert!(
            filled < (pixmap.width() * pixmap.height()) as usize / 2,
            "the injected shape appears to have been drawn"
        );
    }

    #[test]
    fn the_ink_colour_is_the_one_requested() {
        let pixmap = rasterise("Action", 88.0, INK)
            .expect("rasterises")
            .expect("some");
        // Antialiasing dilutes the edges, so look at the most opaque pixel.
        let strongest = pixmap
            .pixels()
            .iter()
            .max_by_key(|p| p.alpha())
            .expect("non-empty");
        assert!(strongest.alpha() > 250, "no solid glyph interior found");
        let demultiplied = strongest.demultiply();
        assert_eq!(
            (
                demultiplied.red(),
                demultiplied.green(),
                demultiplied.blue()
            ),
            (INK.r, INK.g, INK.b)
        );
    }

    #[test]
    fn escaping_covers_every_metacharacter() {
        assert_eq!(escape_xml(r#"&<>"'"#), "&amp;&lt;&gt;&quot;&apos;");
    }
}
