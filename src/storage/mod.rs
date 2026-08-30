//! Object storage for the two cache tiers and specification persistence.
//!
//! Holds **rendered posters and the specifications that produced them, and
//! nothing else**. Source artwork fetched from TMDB is never written here; see
//! `docs/adr/0007-do-not-persist-source-artwork.md`.
//!
//! Backed by `object_store`, which abstracts S3, GCS and Azure behind one
//! trait and ships `LocalFileSystem` and `InMemory` backends. Those two are
//! the reason for the choice: integration tests run against `InMemory` with no
//! containers and no credentials, and local development runs against a
//! directory. See `docs/adr/0003-object-storage-over-redis-for-specs.md`.
//!
//! # Error handling
//!
//! A missing object is not an error. Every read returns `Option`, because a
//! cache miss is the ordinary case on this path — it happens on every first
//! request for a key — and modelling it as an error would put the common path
//! through the failure branch and invite a caller to treat a miss as an
//! outage.

pub mod keys;

use std::sync::Arc;

use bytes::Bytes;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt as _, PutPayload};

use crate::spec::{CacheKey, ResolvedSpec};

/// Why a storage operation failed.
///
/// Deliberately does not include "not found": see the module documentation.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The backend was unreachable or refused the operation.
    #[error("storage backend error: {0}")]
    Backend(#[from] object_store::Error),
    /// A stored specification could not be parsed.
    ///
    /// Distinct from a backend failure because it is not retryable: the object
    /// is present and will parse the same way every time.
    #[error("stored specification at {path} is malformed: {source}")]
    MalformedSpec {
        /// Path of the offending object.
        path: String,
        /// The parse failure.
        source: serde_json::Error,
    },
    /// A stored specification parsed but carried an out-of-range field.
    ///
    /// Separate from [`StorageError::MalformedSpec`] because the object is
    /// well-formed JSON: it was written by a build whose clamp ranges differ
    /// from this one's, which is an operational fact worth distinguishing from
    /// corruption.
    #[error("stored specification at {path} is out of range: {source}")]
    InvalidSpec {
        /// Path of the offending object.
        path: String,
        /// The range violation.
        source: crate::spec::resolved::SpecViolation,
    },
}

/// Cache tiers and specification persistence over an object store.
#[derive(Clone)]
pub struct Storage {
    backend: Arc<dyn ObjectStore>,
}

impl std::fmt::Debug for Storage {
    /// Prints the backend's own description rather than its contents.
    ///
    /// `AppState` derives `Debug` and is attached to tracing spans; a backend
    /// that printed its configuration could put credentials in a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage")
            .field("backend", &self.backend.to_string())
            .finish()
    }
}

impl Storage {
    /// Wraps an object store.
    ///
    /// # Arguments
    ///
    /// * `backend` — any `object_store` implementation.
    #[must_use]
    pub fn new(backend: Arc<dyn ObjectStore>) -> Self {
        Self { backend }
    }

    /// Builds an in-memory store.
    ///
    /// Intended for tests and for a development run that should not persist
    /// anything.
    #[must_use]
    pub fn in_memory() -> Self {
        Self::new(Arc::new(object_store::memory::InMemory::new()))
    }

    /// Reads an object, treating absence as a miss rather than a failure.
    ///
    /// # Arguments
    ///
    /// * `path` — the object path.
    ///
    /// # Returns
    ///
    /// The object's bytes, or `None` if it does not exist.
    ///
    /// # Errors
    ///
    /// [`StorageError::Backend`] for any failure other than absence.
    async fn get(&self, path: &str) -> Result<Option<Bytes>, StorageError> {
        match self.backend.get(&Path::from(path)).await {
            Ok(result) => Ok(Some(result.bytes().await?)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes an object, overwriting any existing one.
    ///
    /// Overwriting is safe for every path this module produces: all three are
    /// content-addressed, so an object being overwritten is being overwritten
    /// with identical bytes.
    ///
    /// # Errors
    ///
    /// [`StorageError::Backend`] if the write fails.
    async fn put(&self, path: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.backend
            .put(&Path::from(path), PutPayload::from(bytes))
            .await?;
        Ok(())
    }

    /// Reads a rendered poster from the L2 tier.
    ///
    /// # Errors
    ///
    /// [`StorageError::Backend`] if the read fails for a reason other than
    /// absence.
    pub async fn get_l2_poster(&self, key: CacheKey) -> Result<Option<Bytes>, StorageError> {
        self.get(&keys::l2_poster(key)).await
    }

    /// Writes a rendered poster to the L2 tier.
    ///
    /// # Errors
    ///
    /// [`StorageError::Backend`] if the write fails.
    pub async fn put_l2_poster(&self, key: CacheKey, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.put(&keys::l2_poster(key), bytes).await
    }

    /// Reads a persisted specification.
    ///
    /// # Errors
    ///
    /// [`StorageError::MalformedSpec`] if the object exists but does not
    /// parse, and [`StorageError::Backend`] for any other failure. The two are
    /// separated because only one of them is worth retrying.
    pub async fn get_spec(&self, key: CacheKey) -> Result<Option<ResolvedSpec>, StorageError> {
        let path = keys::spec(key);
        let Some(bytes) = self.get(&path).await? else {
            return Ok(None);
        };

        let spec: ResolvedSpec =
            serde_json::from_slice(&bytes).map_err(|source| StorageError::MalformedSpec {
                path: path.clone(),
                source,
            })?;

        // Deserialisation bypasses `preset::resolve`, so the clamp invariant
        // has to be re-established here rather than assumed.
        spec.validate()
            .map_err(|source| StorageError::InvalidSpec { path, source })?;

        Ok(Some(spec))
    }

    /// Persists a specification under its own key.
    ///
    /// The key is derived from the specification rather than supplied, so a
    /// specification cannot be stored under a key that does not describe it.
    /// Accepting both as arguments would make that mismatch expressible, and
    /// it would serve one poster's pixels for another poster's URL.
    ///
    /// # Returns
    ///
    /// The key the specification was stored under.
    ///
    /// # Errors
    ///
    /// [`StorageError::Backend`] if the write fails.
    ///
    /// # Panics
    ///
    /// Panics if a [`ResolvedSpec`] fails to serialise. `serde_json` can only
    /// fail on a map with non-string keys or on a type whose `Serialize` impl
    /// returns an error, and `ResolvedSpec` is a flat struct of strings,
    /// enums, floats and integers with derived impls. Returning an error here
    /// instead would add a variant every caller must handle and no caller can
    /// ever observe.
    pub async fn put_spec(&self, spec: &ResolvedSpec) -> Result<CacheKey, StorageError> {
        let key = spec.cache_key();
        let bytes =
            serde_json::to_vec(spec).expect("ResolvedSpec is a flat struct with derived Serialize");
        self.put(&keys::spec(key), bytes).await?;
        Ok(key)
    }

    /// Reports whether the backend is reachable.
    ///
    /// Used by the readiness probe. Performs a listing rather than a read,
    /// because a read of a known path would depend on that object existing and
    /// a fresh bucket would report unready forever.
    ///
    /// # Limitation
    ///
    /// A listing proves the backend answers, not that it holds anything. With
    /// `LocalFileSystem` in particular, a prefix directory that has been
    /// deleted lists as empty and therefore reports reachable. That is
    /// acceptable for the backend this is aimed at -- an S3 bucket that has
    /// gone away returns an error rather than an empty listing -- but it means
    /// the local backend's readiness signal is weaker than the remote one's.
    ///
    /// # Errors
    ///
    /// [`StorageError::Backend`] if the backend cannot be reached.
    pub async fn check_reachable(&self) -> Result<(), StorageError> {
        use futures_util::StreamExt as _;

        // One entry is enough to prove reachability, and an empty bucket is a
        // healthy bucket, so the stream ending immediately is also a pass.
        let mut listing = self.backend.list(Some(&Path::from("spec")));
        match listing.next().await {
            None | Some(Ok(_)) => Ok(()),
            Some(Err(error)) => Err(error.into()),
        }
    }
}
