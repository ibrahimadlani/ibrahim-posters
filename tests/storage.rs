//! Storage round trips against the in-memory backend.
//!
//! No containers and no credentials: `object_store`'s `InMemory` backend is
//! the reason ADR 0003 chose that crate over `aws-sdk-s3`.

mod support;

use poster_service::spec::{preset, request::PosterRequest, CacheKey};
use poster_service::storage::{keys, Storage, StorageError};

use std::sync::Arc;

use futures_util::StreamExt as _;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{ObjectStore as _, ObjectStoreExt as _, PutPayload};

/// Returns a `Storage` alongside the backend it wraps.
///
/// Writing directly to the backend is how these tests plant objects that
/// `Storage`'s own API cannot produce -- a malformed specification, or one
/// carrying values this build's clamp ranges reject. Going through the shared
/// `Arc` keeps that capability in the tests rather than widening the public
/// API to accommodate them.
fn shared() -> (Storage, Arc<InMemory>) {
    let backend = Arc::new(InMemory::new());
    (Storage::new(backend.clone()), backend)
}

/// Plants raw bytes at `path` in the shared backend.
async fn plant(backend: &Arc<InMemory>, path: &str, bytes: Vec<u8>) {
    backend
        .put(&Path::from(path), PutPayload::from(bytes))
        .await
        .expect("write succeeds");
}

fn spec(json: &str) -> poster_service::spec::ResolvedSpec {
    let request: PosterRequest = serde_json::from_str(json).expect("valid request");
    preset::resolve(&request, &support::catalogue()).expect("resolves")
}

fn standard() -> poster_service::spec::ResolvedSpec {
    spec(r#"{ "tmdb_movie_id": 27205 }"#)
}

#[tokio::test]
async fn a_miss_is_none_rather_than_an_error() {
    // The ordinary case on this path: every first request for a key misses.
    // Modelling it as an error would route the common path through the
    // failure branch and invite a caller to treat it as an outage.
    let storage = Storage::in_memory();
    let key = standard().cache_key();

    assert!(storage
        .get_l2_poster(key)
        .await
        .expect("no error")
        .is_none());
    assert!(storage.get_spec(key).await.expect("no error").is_none());
    assert!(storage.get_spec(key).await.expect("no error").is_none());
}

#[tokio::test]
async fn a_specification_round_trips_under_its_own_key() {
    let storage = Storage::in_memory();
    let original = standard();

    let key = storage.put_spec(&original).await.expect("write succeeds");
    assert_eq!(key, original.cache_key(), "stored under a key it describes");

    let loaded = storage
        .get_spec(key)
        .await
        .expect("read succeeds")
        .expect("present");

    assert_eq!(loaded, original);
    assert_eq!(loaded.cache_key(), key, "the key survives the round trip");
}

#[tokio::test]
async fn every_preset_round_trips() {
    let storage = Storage::in_memory();

    for name in ["standard", "cinematic", "minimal", "poster_wall"] {
        let original = spec(&format!(
            r#"{{ "tmdb_movie_id": 27205, "preset": "{name}" }}"#
        ));
        let key = storage.put_spec(&original).await.expect("write succeeds");
        let loaded = storage
            .get_spec(key)
            .await
            .expect("read succeeds")
            .expect("present");
        assert_eq!(loaded, original, "{name} did not survive the round trip");
    }
}

#[tokio::test]
async fn poster_bytes_round_trip() {
    let storage = Storage::in_memory();
    let key = standard().cache_key();
    let bytes = b"pretend this is webp".to_vec();

    storage
        .put_l2_poster(key, bytes.clone())
        .await
        .expect("write succeeds");

    let loaded = storage
        .get_l2_poster(key)
        .await
        .expect("read succeeds")
        .expect("present");

    assert_eq!(&loaded[..], &bytes[..]);
}

#[tokio::test]
async fn source_artwork_has_no_place_in_storage() {
    // The service stores what it produces, not what it consumes. Only two
    // prefixes exist, and neither can hold a fetched image.
    let (storage, backend) = shared();
    let spec = standard();

    let key = storage.put_spec(&spec).await.expect("write succeeds");
    storage
        .put_l2_poster(key, b"pretend this is webp".to_vec())
        .await
        .expect("write succeeds");

    let mut listing = backend.list(None);
    let mut prefixes = std::collections::BTreeSet::new();
    while let Some(entry) = listing.next().await {
        let path = entry.expect("listing succeeds").location.to_string();
        prefixes.insert(
            path.split('/')
                .next()
                .expect("a path has at least one segment")
                .to_owned(),
        );
    }

    assert_eq!(
        prefixes,
        ["l2".to_owned(), "spec".to_owned()].into_iter().collect(),
        "storage holds a prefix other than rendered posters and specifications"
    );
}

#[tokio::test]
async fn writing_twice_is_idempotent() {
    // Every path is content-addressed, so an overwrite writes identical
    // bytes. Singleflight reduces this case but does not eliminate it across
    // processes, so it has to be safe rather than merely rare.
    let storage = Storage::in_memory();
    let original = standard();

    let first = storage.put_spec(&original).await.expect("write succeeds");
    let second = storage.put_spec(&original).await.expect("write succeeds");

    assert_eq!(first, second);
    assert_eq!(
        storage
            .get_spec(first)
            .await
            .expect("read")
            .expect("present"),
        original
    );
}

#[tokio::test]
async fn an_out_of_range_stored_specification_is_rejected() {
    // A specification written by a build with wider clamp ranges parses
    // cleanly but must not be rendered: the renderer was never tested at that
    // geometry. This is why Deserialize alone is not sufficient.
    let (storage, backend) = shared();
    let key = standard().cache_key();

    // Serialised from a real specification and then tampered with, rather
    // than written out field by field: a hand-written fixture has to be
    // updated every time the specification gains a field, and the update is
    // silent guesswork about what the rest of the document should say.
    let mut tampered = serde_json::to_value(standard()).expect("serialises");
    tampered["blur_sigma"] = serde_json::json!(9000.0);

    plant(
        &backend,
        &keys::spec(key),
        serde_json::to_vec(&tampered).expect("serialises"),
    )
    .await;

    let error = storage
        .get_spec(key)
        .await
        .expect_err("an out-of-range specification must be rejected");

    assert!(
        matches!(error, StorageError::InvalidSpec { source, .. } if source.field == "blur_sigma"),
        "got {error:?}"
    );
}

#[tokio::test]
async fn malformed_json_at_a_spec_path_is_rejected() {
    let (storage, backend) = shared();
    let key = standard().cache_key();

    plant(&backend, &keys::spec(key), b"{ not json".to_vec()).await;

    let error = storage
        .get_spec(key)
        .await
        .expect_err("malformed json must be rejected");

    assert!(
        matches!(error, StorageError::MalformedSpec { .. }),
        "got {error:?}"
    );
}

#[tokio::test]
async fn an_empty_backend_reports_reachable() {
    // Readiness must not depend on any object existing, or a fresh bucket
    // would report unready forever.
    let storage = Storage::in_memory();
    storage.check_reachable().await.expect("empty is reachable");
}

#[tokio::test]
async fn a_populated_backend_reports_reachable() {
    let storage = Storage::in_memory();
    storage.put_spec(&standard()).await.expect("write succeeds");
    storage
        .check_reachable()
        .await
        .expect("populated is reachable");
}

#[tokio::test]
async fn distinct_specifications_do_not_share_storage() {
    let storage = Storage::in_memory();

    let a = standard();
    let b = spec(r#"{ "tmdb_movie_id": 27205, "preset": "minimal" }"#);

    let key_a = storage.put_spec(&a).await.expect("write succeeds");
    let key_b = storage.put_spec(&b).await.expect("write succeeds");

    assert_ne!(key_a, key_b);
    assert_eq!(
        storage
            .get_spec(key_a)
            .await
            .expect("read")
            .expect("present"),
        a
    );
    assert_eq!(
        storage
            .get_spec(key_b)
            .await
            .expect("read")
            .expect("present"),
        b
    );
}

#[tokio::test]
async fn a_key_parsed_from_hex_addresses_the_same_object() {
    // The GET handler receives the key as text from a URL; it must address
    // the object the POST handler wrote.
    let storage = Storage::in_memory();
    let key = storage.put_spec(&standard()).await.expect("write succeeds");

    let round_tripped = CacheKey::from_hex(&key.to_hex()).expect("valid hex");
    assert!(storage
        .get_spec(round_tripped)
        .await
        .expect("read succeeds")
        .is_some());
}
