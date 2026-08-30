//! Enforces the render module boundary.
//!
//! `src/render/` must depend only on a resolved specification and byte
//! buffers: no async, no HTTP, no storage. Three properties depend on it —
//! rendering is unit-testable without a runtime, two renderer versions can be
//! compared pixel by pixel, and `rayon` can be used inside a render without
//! risk of blocking a tokio worker.
//!
//! This lives in a test rather than in `clippy.toml` because clippy
//! configuration applies to the whole crate. A `disallowed-types` entry for
//! `reqwest::Client` would also fire in `state.rs`, where the client belongs.
//! A test can scope the check to one directory, which is what the rule
//! actually means.

use std::fs;
use std::path::Path;

/// Constructs that would drag async or I/O into the renderer, with the reason
/// each one is refused. Matched against source text, which is coarse but
/// sufficient: the failure this guards against is someone reaching for
/// `reqwest` inside a render stage, not someone deliberately evading a grep.
const FORBIDDEN: &[(&str, &str)] = &[
    ("reqwest", "render/ must not perform I/O; pass byte buffers in"),
    ("object_store", "render/ must not read storage; pass byte buffers in"),
    ("axum", "render/ must not know about HTTP"),
    ("async fn", "render/ must stay synchronous"),
    (".await", "render/ must stay synchronous"),
    ("tokio::", "render/ must not depend on a runtime"),
];

#[test]
fn render_module_has_no_io_or_async() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/render");

    // The renderer arrives in milestone M4. Until then there is nothing to
    // check, and the test passing vacuously is the correct outcome.
    if !root.exists() {
        return;
    }

    let mut violations = Vec::new();
    visit(&root, &mut violations);

    assert!(
        violations.is_empty(),
        "render module boundary violated:\n{}",
        violations.join("\n")
    );
}

/// Walks `dir` recursively, appending one message per violation found.
fn visit(dir: &Path, violations: &mut Vec<String>) {
    let entries = fs::read_dir(dir).expect("render directory is readable");

    for entry in entries {
        let path = entry.expect("directory entry is readable").path();

        if path.is_dir() {
            visit(&path, violations);
            continue;
        }

        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }

        let source = fs::read_to_string(&path).expect("source file is readable");

        for (line_number, line) in source.lines().enumerate() {
            // Comments describe the boundary rather than cross it; the module
            // documentation names these very identifiers.
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }

            for (needle, reason) in FORBIDDEN {
                if line.contains(needle) {
                    violations.push(format!(
                        "  {}:{}: `{needle}` — {reason}",
                        path.display(),
                        line_number + 1
                    ));
                }
            }
        }
    }
}
