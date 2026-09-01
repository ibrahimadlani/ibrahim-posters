# 9. Derive badge colours from the artwork

Date: 2026-09-01

## Status

Accepted.

## Context

The badge palette was three fixed pairs — white on dark, dark on yellow, and a
stroked outline — chosen once and applied to every poster. That is defensible
and it is what most systems do, but it produces a badge that belongs to the
service rather than to the poster: the same yellow pill over a yellow poster
and over a blue one.

A set of reference posters was supplied as the target treatment. Measuring
them showed the badge fill was not fixed at all. Across five posters it
tracked the dominant colour of each poster's top region within a few percent
in hue and lightness, and the text colour flipped between black and white in a
way that a plain lightness threshold does not reproduce: a mid-sage badge took
white text while a lighter warm grey took black, and the two are close enough
in HSL lightness that the threshold would have to sit between 37 and 58 with
nothing to justify where.

Three candidate rules were checked against the measurements.

**Average colour of the poster.** Rejected. Complementary regions cancel and
nearly every poster averages to a desaturated brown, which matched none of the
five.

**Mode of the raw pixels.** Rejected. Posters are mostly near-black — vignettes,
shadows, letterboxing — so the mode returns black for most of them.

**Mode with the extremes excluded.** Accepted. Discarding pixels outside a
mean-channel band of 24–232 before binning reproduced all five fills.

For the text, maximising WCAG contrast rather than testing a threshold got all
five right, and it is the rule that explains *why* the sage and the grey differ:
relative luminance weights green six times more than blue, so two colours can
share a lightness and not a luminance.

The same measurement pass showed two further treatments the fixed palette had
no way to express — an inset shadow under the top edge, and a bottom band that
blends toward a dark warm neutral rather than toward black. Both are needed for
the derived badge to be legible: a colour taken *from* the artwork has, by
construction, poor contrast *against* it.

## Decision

Under a preset that asks for it, the badge takes its fill from the dominant
colour of the artwork's top 45%, computed as the mode of a 16-level-per-channel
histogram with near-black and near-white excluded, reported as the mean of the
winning bin rather than the bin centre. Its text is whichever of black and
white has the higher WCAG 2.1 contrast ratio against that fill.

The colour is sampled from the resized background *before* the inset shadow is
applied. Sampling afterwards would darken the badge in step with the shadow
instead of matching the poster.

This is a preset property, not a request field, and only `standard` sets it.
The other presets keep their fixed palettes.

## Consequences

The badge belongs to the poster. On a wall of them the effect is that each
badge reads as part of its artwork rather than as a layer over it.

Contrast is guaranteed against the badge but not against the artwork behind it,
which is what the inset shadow exists to fix. The two ship together and the
preset that sets one sets the other.

The mode is computed on every cold render, over every second pixel of the top
45%. At w1000 that is roughly 84,000 samples into a hash map — cheap beside the
resize, and it does not run at all on a cache hit.

There is no way for a caller to ask for a specific badge colour. That is
deliberate: a caller who could set the fill could also set it to something
unreadable, and the contrast rule would then be advisory rather than
guaranteed. If a request-level override is ever wanted, it should carry the
fill only and keep deriving the ink.

Two posters whose artwork differs only slightly can land in different histogram
bins and get visibly different badges. The bins are coarse enough that this is
rare, and the alternative — smoothing across bins — reintroduces the averaging
this rule exists to avoid.
