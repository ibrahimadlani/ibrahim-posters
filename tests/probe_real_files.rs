//! Probes real encoded files, not hand-built headers.
//!
//! The unit tests in `tmdb::probe` construct headers from the specification,
//! which proves the parser matches *this author's reading* of the format. It
//! does not prove it matches what an encoder actually emits — and the two
//! diverge in exactly the places that matter: the JPEG fixture below carries a
//! JFIF APP0 segment before its SOF, so a parser that read a constant offset
//! would pass every unit test and fail here.
//!
//! Fixtures are 37x53, deliberately not round numbers, so a parser that
//! returned a plausible constant would be caught.

use poster_service::tmdb::probe::{self, ImageFormat};

/// Dimensions every fixture declares.
const EXPECTED: (u32, u32) = (37, 53);

#[test]
fn reads_a_real_jpeg() {
    let bytes = include_bytes!("fixtures/probe_37x53.jpg");
    let probed = probe::probe(bytes).expect("real jpeg parses");

    assert_eq!((probed.width, probed.height), EXPECTED);
    assert_eq!(probed.format, ImageFormat::Jpeg);
}

#[test]
fn reads_a_real_png() {
    let bytes = include_bytes!("fixtures/probe_37x53.png");
    let probed = probe::probe(bytes).expect("real png parses");

    assert_eq!((probed.width, probed.height), EXPECTED);
    assert_eq!(probed.format, ImageFormat::Png);
}

#[test]
fn reads_a_real_webp() {
    // Encoded by the same libwebp build the service encodes with, so this
    // also pins the assumption that our own output is probe-able.
    let bytes = include_bytes!("fixtures/probe_37x53.webp");
    let probed = probe::probe(bytes).expect("real webp parses");

    assert_eq!((probed.width, probed.height), EXPECTED);
    assert_eq!(probed.format, ImageFormat::WebP);
}

#[test]
fn dimensions_are_readable_from_the_header_alone() {
    // The guard's whole value is rejecting before the body arrives. Assert
    // each format is decidable from a short prefix rather than the full file.
    let cases: [(&[u8], usize); 3] = [
        (include_bytes!("fixtures/probe_37x53.png"), 24),
        (include_bytes!("fixtures/probe_37x53.webp"), 30),
        (include_bytes!("fixtures/probe_37x53.jpg"), 512),
    ];

    for (bytes, prefix) in cases {
        let probed = probe::probe(&bytes[..prefix.min(bytes.len())])
            .expect("dimensions readable from prefix");
        assert_eq!((probed.width, probed.height), EXPECTED);
    }
}

#[test]
fn every_prefix_of_a_real_file_is_handled_without_panicking() {
    // The fetch probes as bytes arrive, so it observes every prefix in turn.
    for bytes in [
        include_bytes!("fixtures/probe_37x53.jpg").as_slice(),
        include_bytes!("fixtures/probe_37x53.png").as_slice(),
        include_bytes!("fixtures/probe_37x53.webp").as_slice(),
    ] {
        for len in 0..=bytes.len() {
            let _ = probe::probe(&bytes[..len]);
        }
    }
}
