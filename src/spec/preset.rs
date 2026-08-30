//! The preset catalogue and the merge that turns a request into a
//! specification.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::spec::clamp;
use crate::spec::request::{PosterRequest, SpecError};
use crate::spec::resolved::ResolvedSpec;

/// The catalogue source, embedded so that a deployment cannot disagree with
/// the binary about what `"standard"` means.
const CATALOGUE_SOURCE: &str = include_str!("../../assets/presets.toml");

/// A named layout preset.
///
/// Values are authored at w1000 and scaled at resolution time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    /// Height of the blurred band as a fraction of poster height.
    pub blur_band_fraction: f32,
    /// Gaussian sigma at w1000, in pixels.
    pub blur_sigma: f32,
    /// Peak opacity of the darkening ramp.
    pub darken_strength: f32,
    /// Logo width as a fraction of poster width.
    pub logo_width_fraction: f32,
    /// Distance from the bottom edge to the logo, as a fraction of height.
    pub logo_bottom_fraction: f32,
    /// Badge row height in pixels at w1000.
    pub badge_height: u32,
}

/// Returns the parsed preset catalogue.
///
/// Parsed once on first use and cached. A `BTreeMap` rather than a `HashMap`
/// so that [`catalogue_names`] and the `/v1/presets` response are ordered
/// deterministically — an unordered catalogue would make the snapshot test of
/// that endpoint flap between runs.
///
/// # Panics
///
/// Panics if the embedded catalogue does not parse. This is a compile-time
/// constant, so a failure here means the binary is malformed and there is no
/// recovery worth attempting; the `catalogue_parses` test makes it a build
/// failure rather than a startup failure.
fn catalogue() -> &'static BTreeMap<String, Preset> {
    static CATALOGUE: OnceLock<BTreeMap<String, Preset>> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        toml::from_str(CATALOGUE_SOURCE).expect("embedded preset catalogue is valid TOML")
    })
}

/// Looks a preset up by name.
///
/// # Arguments
///
/// * `name` — the preset name, as supplied by the caller.
///
/// # Returns
///
/// The preset, or `None` if the catalogue has no entry under that name.
///
/// # Examples
///
/// ```
/// assert!(poster_service::spec::preset::lookup("standard").is_some());
/// assert!(poster_service::spec::preset::lookup("nonexistent").is_none());
/// ```
#[must_use]
pub fn lookup(name: &str) -> Option<Preset> {
    catalogue().get(name).copied()
}

/// Returns every preset in the catalogue, ordered by name.
#[must_use]
pub fn catalogue_entries() -> Vec<(&'static str, Preset)> {
    catalogue()
        .iter()
        .map(|(name, preset)| (name.as_str(), *preset))
        .collect()
}

impl Preset {
    /// Merges a request and its overrides into a canonical specification.
    ///
    /// Order of operations is the point of this function: **merge, then
    /// clamp**. Clamping an override before merging would let two requests
    /// that clamp to the same value survive as distinct specifications, which
    /// splits one cache entry into two and costs a render on every request
    /// that would otherwise have hit.
    ///
    /// Geometry is scaled by [`OutputWidth::scale`] *after* clamping, so the
    /// clamp ranges are stated once at w1000 rather than once per output size.
    ///
    /// # Arguments
    ///
    /// * `request` — the request to resolve against this preset.
    ///
    /// # Returns
    ///
    /// A [`ResolvedSpec`] satisfying every invariant documented on that type.
    ///
    /// # Errors
    ///
    /// Propagates badge validation failures from
    /// [`PosterRequest::normalised_badges`].
    pub fn resolve(&self, request: &PosterRequest) -> Result<ResolvedSpec, SpecError> {
        let badges = request.normalised_badges()?;
        let overrides = request.overrides;
        let scale = request.width.scale();

        Ok(ResolvedSpec {
            source: request.source.clone(),
            source_kind: request.source_kind,
            logo: request.logo.clone(),
            badges,
            width: request.width,
            blur_band_fraction: clamp::f32_into(
                overrides
                    .blur_band_fraction
                    .unwrap_or(self.blur_band_fraction),
                clamp::BLUR_BAND_FRACTION,
            ),
            // Sigma is a pixel measurement, so it scales with the output. The
            // fractions above are already relative and must not be scaled.
            blur_sigma: clamp::f32_into(
                overrides.blur_sigma.unwrap_or(self.blur_sigma),
                clamp::BLUR_SIGMA,
            ) * scale,
            darken_strength: clamp::f32_into(
                overrides.darken_strength.unwrap_or(self.darken_strength),
                clamp::DARKEN_STRENGTH,
            ),
            logo_width_fraction: clamp::f32_into(
                overrides
                    .logo_width_fraction
                    .unwrap_or(self.logo_width_fraction),
                clamp::LOGO_WIDTH_FRACTION,
            ),
            logo_bottom_fraction: clamp::f32_into(
                overrides
                    .logo_bottom_fraction
                    .unwrap_or(self.logo_bottom_fraction),
                clamp::LOGO_BOTTOM_FRACTION,
            ),
            badge_height: clamp::u32_into(
                overrides.badge_height.unwrap_or(self.badge_height),
                clamp::BADGE_HEIGHT,
            ) * request.width.pixel_scale(),
        })
    }
}

/// Resolves a request against the preset it names.
///
/// # Arguments
///
/// * `request` — the validated wire request.
///
/// # Returns
///
/// The resolved, clamped, canonical specification.
///
/// # Errors
///
/// [`SpecError::UnknownPreset`] if the named preset is not in the catalogue,
/// or any badge validation failure.
pub fn resolve(request: &PosterRequest) -> Result<ResolvedSpec, SpecError> {
    let preset =
        lookup(&request.preset).ok_or_else(|| SpecError::UnknownPreset(request.preset.clone()))?;
    preset.resolve(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::request::{OutputWidth, Overrides};
    use crate::tmdb::PosterPath;

    fn request() -> PosterRequest {
        PosterRequest {
            source: PosterPath::parse("/kqjL17yufvn9OVLyXYpvtyrFfak.jpg").expect("valid"),
            source_kind: crate::tmdb::SourceKind::Poster,
            preset: "standard".to_owned(),
            logo: None,
            badges: Vec::new(),
            width: OutputWidth::W1000,
            overrides: Overrides::default(),
        }
    }

    #[test]
    fn catalogue_parses_and_is_non_empty() {
        assert!(!catalogue_entries().is_empty());
        assert!(lookup("standard").is_some());
    }

    #[test]
    fn every_catalogue_entry_is_already_within_its_clamp_range() {
        // A preset outside its own range would be silently corrected at
        // resolution time, so the catalogue would not describe what it renders.
        for (name, preset) in catalogue_entries() {
            assert!(
                clamp::BLUR_BAND_FRACTION.contains(&preset.blur_band_fraction),
                "{name}: blur_band_fraction out of range"
            );
            assert!(
                clamp::BLUR_SIGMA.contains(&preset.blur_sigma),
                "{name}: blur_sigma out of range"
            );
            assert!(
                clamp::DARKEN_STRENGTH.contains(&preset.darken_strength),
                "{name}: darken_strength out of range"
            );
            assert!(
                clamp::LOGO_WIDTH_FRACTION.contains(&preset.logo_width_fraction),
                "{name}: logo_width_fraction out of range"
            );
            assert!(
                clamp::LOGO_BOTTOM_FRACTION.contains(&preset.logo_bottom_fraction),
                "{name}: logo_bottom_fraction out of range"
            );
            assert!(
                clamp::BADGE_HEIGHT.contains(&preset.badge_height),
                "{name}: badge_height out of range"
            );
        }
    }

    #[test]
    fn none_inherits_from_the_preset_rather_than_zeroing() {
        let resolved = resolve(&request()).expect("resolves");
        let preset = lookup("standard").expect("present");
        assert!((resolved.blur_sigma - preset.blur_sigma).abs() < f32::EPSILON);
    }

    #[test]
    fn an_override_beats_the_preset() {
        let mut req = request();
        req.overrides.blur_sigma = Some(8.0);
        let resolved = resolve(&req).expect("resolves");
        assert!((resolved.blur_sigma - 8.0).abs() < f32::EPSILON);
    }

    #[test]
    fn overrides_are_clamped_after_merging() {
        let mut req = request();
        req.overrides.blur_sigma = Some(10_000.0);
        let resolved = resolve(&req).expect("resolves");
        assert!((resolved.blur_sigma - *clamp::BLUR_SIGMA.end()).abs() < f32::EPSILON);
    }

    #[test]
    fn pixel_fields_scale_with_output_width_and_fractions_do_not() {
        let mut req = request();
        req.width = OutputWidth::W2000;
        let big = resolve(&req).expect("resolves");
        let small = resolve(&request()).expect("resolves");

        assert!(
            (big.blur_sigma - small.blur_sigma * 2.0).abs() < f32::EPSILON,
            "sigma is a pixel measurement and must scale"
        );
        assert_eq!(big.badge_height, small.badge_height * 2);
        assert!(
            (big.blur_band_fraction - small.blur_band_fraction).abs() < f32::EPSILON,
            "fractions are already relative and must not scale"
        );
    }

    #[test]
    fn an_unknown_preset_is_reported_by_name() {
        let mut req = request();
        req.preset = "nope".to_owned();
        assert_eq!(
            resolve(&req),
            Err(SpecError::UnknownPreset("nope".to_owned()))
        );
    }
}
