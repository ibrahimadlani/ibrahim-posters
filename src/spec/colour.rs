//! An opaque 24-bit colour, parsed from the `#rrggbb` notation presets use.
//!
//! This lives in `spec` rather than `render` because presets carry colours and
//! `render` depends on `spec`, never the reverse. Putting it the other way
//! round would make the preset catalogue depend on the renderer.

use std::fmt;
use std::str::FromStr;

use serde::de::{Deserialize, Deserializer, Error as _};
use serde::Serialize;

/// An opaque colour with 8 bits per channel.
///
/// Alpha is deliberately absent: every colour a preset names is composited
/// through a ramp that supplies the opacity, so a colour carrying its own
/// alpha would give two places to express the same thing and let them
/// disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(into = "String")]
pub struct Rgb {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl Rgb {
    /// Pure black, the colour a band darkens toward when a preset names none.
    pub const BLACK: Self = Self::new(0, 0, 0);
    /// Pure white.
    pub const WHITE: Self = Self::new(255, 255, 255);

    /// Builds a colour from its three channels.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Returns the channels in the order the cache key encodes them.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 3] {
        [self.r, self.g, self.b]
    }

    /// Returns the WCAG 2.1 relative luminance, in `0.0..=1.0`.
    ///
    /// Used to choose between light and dark text. A plain channel average
    /// would get that choice wrong on saturated colours: pure blue and pure
    /// yellow average alike but differ by a factor of twenty in perceived
    /// brightness, and only one of them takes white text.
    #[must_use]
    pub fn luminance(self) -> f32 {
        0.2126 * linearise(self.r) + 0.7152 * linearise(self.g) + 0.0722 * linearise(self.b)
    }

    /// Returns the WCAG contrast ratio between two colours, in `1.0..=21.0`.
    #[must_use]
    pub fn contrast(self, other: Self) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        let (lighter, darker) = if a > b { (a, b) } else { (b, a) };
        (lighter + 0.05) / (darker + 0.05)
    }
}

/// Converts one channel to linear light, per the sRGB transfer function.
fn linearise(channel: u8) -> f32 {
    let value = f32::from(channel) / 255.0;
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl From<Rgb> for String {
    fn from(value: Rgb) -> Self {
        value.to_string()
    }
}

/// Why parsing can fail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseColourError {
    /// The string was not `#` followed by six hexadecimal digits.
    #[error("expected a colour like #1a2b3c, got {0:?}")]
    Malformed(String),
}

impl FromStr for Rgb {
    type Err = ParseColourError;

    /// Parses `#rrggbb`.
    ///
    /// Only the six-digit form is accepted. The three-digit shorthand is
    /// rejected rather than expanded, because a preset is authored once and
    /// read often, and `#fff` and `#ffffff` reading differently in different
    /// tools is a class of surprise not worth the four saved keystrokes.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let malformed = || ParseColourError::Malformed(text.to_owned());
        let digits = text.strip_prefix('#').ok_or_else(malformed)?;
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(malformed());
        }
        let channel = |range: std::ops::Range<usize>| {
            u8::from_str_radix(&digits[range], 16).map_err(|_| malformed())
        };
        Ok(Self::new(channel(0..2)?, channel(2..4)?, channel(4..6)?))
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_six_digit_form_round_trips() {
        for text in ["#000000", "#ffffff", "#2b0503", "#e8cb44"] {
            let colour: Rgb = text.parse().expect("valid");
            assert_eq!(colour.to_string(), text);
        }
    }

    #[test]
    fn channels_land_in_the_right_order() {
        let colour: Rgb = "#123456".parse().expect("valid");
        assert_eq!((colour.r, colour.g, colour.b), (0x12, 0x34, 0x56));
    }

    #[test]
    fn malformed_colours_are_rejected() {
        for text in [
            "", "#fff", "123456", "#12345", "#1234567", "#12345g", "#ABCDEG",
        ] {
            assert!(text.parse::<Rgb>().is_err(), "{text:?} should not parse");
        }
    }

    #[test]
    fn uppercase_hex_parses_and_normalises() {
        // Accepted on the way in, emitted lowercase, so two spellings of one
        // colour cannot produce two cache keys.
        let colour: Rgb = "#AABBCC".parse().expect("valid");
        assert_eq!(colour.to_string(), "#aabbcc");
    }

    #[test]
    fn luminance_orders_colours_by_perceived_brightness() {
        assert!(Rgb::BLACK.luminance() < 0.001);
        assert!(Rgb::WHITE.luminance() > 0.999);
        // Yellow reads far brighter than blue despite the same channel sum.
        let yellow = Rgb::new(255, 255, 0);
        let blue = Rgb::new(0, 0, 255);
        assert!(yellow.luminance() > blue.luminance() * 10.0);
    }

    #[test]
    fn contrast_is_symmetric_and_bounded() {
        assert!((Rgb::BLACK.contrast(Rgb::WHITE) - 21.0).abs() < 0.01);
        assert!((Rgb::WHITE.contrast(Rgb::BLACK) - 21.0).abs() < 0.01);
        assert!((Rgb::BLACK.contrast(Rgb::BLACK) - 1.0).abs() < 0.001);
    }

    #[test]
    fn deserialises_from_a_toml_string() {
        #[derive(serde::Deserialize)]
        struct Holder {
            tint: Rgb,
        }
        let holder: Holder = toml::from_str(r##"tint = "#2b0503""##).expect("valid");
        assert_eq!(holder.tint, Rgb::new(0x2b, 0x05, 0x03));
        assert!(toml::from_str::<Holder>(r#"tint = "nope""#).is_err());
    }
}
