//! Admission control and single-flight coalescing.
//!
//! Two mechanisms, solving two different problems.
//!
//! **Admission** bounds how many renders run at once. Renders are CPU-bound,
//! so the limit is the core count: admitting more work than there are cores
//! raises latency without raising throughput.
//!
//! **Single-flight** bounds how many callers do the *same work at once*. It is
//! keyed by storage path rather than by poster key, so it covers all three
//! cache tiers with one mechanism.
//!
//! Covering L1 as well as L2 is not incidental. Four concurrent requests for
//! four *different* posters built from one piece of artwork share no poster
//! key, so coalescing on the poster key alone leaves them fetching that
//! artwork four times and resizing it four times — the same thundering herd
//! one layer down, on the stage measurement puts at 12.4 ms. Keying on the
//! path each tier already computes collapses both cases without a second
//! mechanism.

use std::sync::{Arc, Weak};
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

/// How long a request waits for a render slot before being rejected.
///
/// `PLAN.md` originally specified `try_acquire`, which rejects the instant
/// every permit is taken. At 20 req/s and roughly 60 ms renders, mean
/// concurrency is about 1.2; on an eight-core box a bare `try_acquire` only
/// rejects when eight renders are already in flight — a burst that would drain
/// in well under 100 ms, comfortably inside the 250 ms p99 budget. Rejecting
/// it converts a satisfiable request into a 503.
///
/// A bounded wait is still bounded — the unbounded queue `try_acquire` exists
/// to prevent cannot form — but it absorbs the bursts that are the actual
/// traffic shape. Fifty milliseconds is the largest wait that still leaves a
/// full render inside the p99 target with margin. If queueing exceeds it, the
/// answer is more cores, not a longer queue.
pub const ADMISSION_TIMEOUT: Duration = Duration::from_millis(50);

/// How long a follower waits for the leader's render before rendering itself.
///
/// Generous relative to a render, because waiting is the cheap outcome: a
/// follower that gives up early does redundant work, while one that waits a
/// little too long costs only latency it would have spent rendering anyway.
const FOLLOWER_TIMEOUT: Duration = Duration::from_secs(10);

/// Bounds concurrent renders and coalesces duplicate work.
#[derive(Debug, Clone)]
pub struct Admission {
    slots: Arc<Semaphore>,
    inflight: Arc<DashMap<String, Weak<Notify>>>,
    capacity: usize,
}

/// A held render slot.
///
/// Releasing happens on drop, so a render that fails or panics does not leak a
/// permit.
#[derive(Debug)]
pub struct Slot(#[allow(dead_code)] OwnedSemaphorePermit);

/// The caller's role in a single-flight group.
#[derive(Debug)]
pub enum Role {
    /// This caller renders. The guard notifies waiters when dropped.
    Leader(LeaderGuard),
    /// Another caller is already rendering this key.
    Follower(Arc<Notify>),
}

/// Held by the caller that is rendering a key.
///
/// On drop it removes the map entry and wakes every waiter, so a leader that
/// fails or panics releases its followers rather than stranding them. The
/// followers then re-read the cache, find nothing, and render — which is the
/// behaviour without single-flight, and the correct degradation.
#[derive(Debug)]
pub struct LeaderGuard {
    path: String,
    notify: Arc<Notify>,
    inflight: Arc<DashMap<String, Weak<Notify>>>,
}

impl Drop for LeaderGuard {
    fn drop(&mut self) {
        self.inflight.remove(&self.path);
        self.notify.notify_waiters();
    }
}

impl Admission {
    /// Builds admission control sized to the machine.
    ///
    /// # Arguments
    ///
    /// * `capacity` — concurrent renders permitted. Zero is treated as one,
    ///   since a limit of zero would reject every request forever.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            slots: Arc::new(Semaphore::new(capacity)),
            inflight: Arc::new(DashMap::new()),
            capacity,
        }
    }

    /// Builds admission control sized to the available parallelism.
    #[must_use]
    pub fn for_this_machine() -> Self {
        Self::new(num_cpus::get())
    }

    /// Returns the configured concurrency limit.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns how many render slots are currently free.
    #[must_use]
    pub fn available(&self) -> usize {
        self.slots.available_permits()
    }

    /// Returns how many keys are currently being rendered.
    #[must_use]
    pub fn inflight_keys(&self) -> usize {
        self.inflight.len()
    }

    /// Waits up to [`ADMISSION_TIMEOUT`] for a render slot.
    ///
    /// # Returns
    ///
    /// A [`Slot`] that releases on drop, or `None` if the wait expired.
    pub async fn acquire(&self) -> Option<Slot> {
        tokio::time::timeout(ADMISSION_TIMEOUT, Arc::clone(&self.slots).acquire_owned())
            .await
            .ok()?
            .ok()
            .map(Slot)
    }

    /// Joins the single-flight group for a storage path.
    ///
    /// # Arguments
    ///
    /// * `path` — the object being produced, as the storage layer names it.
    ///   Using the storage path rather than a domain key is what lets one
    ///   mechanism cover rendered posters, resized sources and logos.
    ///
    /// # Returns
    ///
    /// [`Role::Leader`] if this caller should do the work, or
    /// [`Role::Follower`] with the handle to await.
    #[must_use]
    pub fn join(&self, path: impl Into<String>) -> Role {
        let path = path.into();

        // The entry API holds the shard lock across the check and the insert,
        // so two callers arriving together cannot both become leader.
        match self.inflight.entry(path.clone()) {
            dashmap::Entry::Occupied(mut occupied) => {
                if let Some(notify) = occupied.get().upgrade() {
                    return Role::Follower(notify);
                }
                // The weak reference is dangling: a previous leader's guard was
                // dropped without its entry being removed, which happens only
                // under concurrent mutation. Take over.
                let notify = Arc::new(Notify::new());
                occupied.insert(Arc::downgrade(&notify));
                Role::Leader(self.guard(path, notify))
            }
            dashmap::Entry::Vacant(vacant) => {
                let notify = Arc::new(Notify::new());
                vacant.insert(Arc::downgrade(&notify));
                Role::Leader(self.guard(path, notify))
            }
        }
    }

    fn guard(&self, path: String, notify: Arc<Notify>) -> LeaderGuard {
        LeaderGuard {
            path,
            notify,
            inflight: Arc::clone(&self.inflight),
        }
    }
}

/// Waits for a leader to finish.
///
/// # Arguments
///
/// * `notify` — the handle from [`Role::Follower`].
///
/// # Returns
///
/// `true` if the leader signalled, `false` if the wait expired.
///
/// A `false` return is not an error. The caller re-reads the cache and renders
/// if it is still empty, which is exactly the behaviour without single-flight.
/// Treating it as a failure would turn a slow leader into a failed request for
/// everyone waiting on it.
pub async fn wait_for_leader(notify: &Notify) -> bool {
    tokio::time::timeout(FOLLOWER_TIMEOUT, notify.notified())
        .await
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> String {
        format!("l2/{byte:02x}.webp")
    }

    #[tokio::test]
    async fn a_slot_is_released_when_it_is_dropped() {
        let admission = Admission::new(1);

        let slot = admission.acquire().await.expect("first acquires");
        assert_eq!(admission.available(), 0);

        drop(slot);
        assert_eq!(admission.available(), 1);
        assert!(admission.acquire().await.is_some(), "slot was not returned");
    }

    #[tokio::test]
    async fn admission_rejects_once_the_wait_expires() {
        let admission = Admission::new(1);
        let _held = admission.acquire().await.expect("first acquires");

        let started = std::time::Instant::now();
        assert!(
            admission.acquire().await.is_none(),
            "a second caller was admitted with no slot free"
        );

        // The bound is the point: it waits, but not indefinitely.
        assert!(started.elapsed() >= ADMISSION_TIMEOUT);
        assert!(started.elapsed() < ADMISSION_TIMEOUT * 4);
    }

    #[tokio::test]
    async fn a_waiting_caller_is_admitted_when_a_slot_frees() {
        // The behaviour try_acquire would not have: a burst that drains
        // inside the timeout is served rather than rejected.
        let admission = Admission::new(1);
        let held = admission.acquire().await.expect("first acquires");

        let releaser = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            drop(held);
        });

        assert!(
            admission.acquire().await.is_some(),
            "a caller was rejected although a slot freed within the wait"
        );
        releaser.await.expect("releaser completes");
    }

    #[tokio::test]
    async fn capacity_is_never_zero() {
        // A limit of zero would reject every request forever.
        assert_eq!(Admission::new(0).capacity(), 1);
        assert!(Admission::new(0).acquire().await.is_some());
    }

    #[tokio::test]
    async fn the_first_caller_for_a_key_leads_and_the_rest_follow() {
        let admission = Admission::new(4);

        let leader = admission.join(key(1));
        assert!(matches!(leader, Role::Leader(_)));

        for _ in 0..3 {
            assert!(
                matches!(admission.join(key(1)), Role::Follower(_)),
                "a second caller for one key became a leader"
            );
        }
        assert_eq!(admission.inflight_keys(), 1);
    }

    #[tokio::test]
    async fn different_keys_do_not_block_each_other() {
        let admission = Admission::new(4);

        // Both roles are bound: a guard left as a temporary drops at the end
        // of the statement and removes its own entry, which would make the
        // count below read 1 for reasons unrelated to what is being tested.
        let _first = admission.join(key(1));
        let second = admission.join(key(2));

        assert!(
            matches!(second, Role::Leader(_)),
            "an unrelated key was made to wait"
        );
        assert_eq!(admission.inflight_keys(), 2);
    }

    #[tokio::test]
    async fn the_map_empties_when_leaders_finish() {
        // A map that grew without bound under key churn would be a slow leak.
        let admission = Admission::new(4);
        {
            let _a = admission.join(key(1));
            let _b = admission.join(key(2));
            assert_eq!(admission.inflight_keys(), 2);
        }
        assert_eq!(
            admission.inflight_keys(),
            0,
            "entries outlived their leaders"
        );
    }

    #[tokio::test]
    async fn a_follower_is_woken_when_the_leader_finishes() {
        let admission = Admission::new(4);
        let leader = admission.join(key(1));

        let Role::Follower(notify) = admission.join(key(1)) else {
            panic!("expected to follow");
        };

        let waiter = tokio::spawn(async move { wait_for_leader(&notify).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        drop(leader);

        assert!(
            waiter.await.expect("waiter completes"),
            "follower was not woken"
        );
    }

    #[tokio::test]
    async fn a_leader_that_fails_still_releases_its_followers() {
        // The guard notifies on drop, so a panicking or erroring leader does
        // not strand everyone waiting on it.
        let admission = Admission::new(4);
        let Role::Leader(guard) = admission.join(key(1)) else {
            panic!("expected to lead");
        };
        let Role::Follower(notify) = admission.join(key(1)) else {
            panic!("expected to follow");
        };

        let waiter = tokio::spawn(async move { wait_for_leader(&notify).await });
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Simulates a render that returned an error: the guard is dropped
        // without anything having been written to the cache.
        drop(guard);

        assert!(waiter.await.expect("waiter completes"));
        assert_eq!(admission.inflight_keys(), 0);
    }

    #[tokio::test]
    async fn a_key_can_be_led_again_after_its_leader_finishes() {
        let admission = Admission::new(4);
        drop(admission.join(key(1)));
        let again = admission.join(key(1));
        assert!(
            matches!(again, Role::Leader(_)),
            "the key stayed locked after its leader finished"
        );
    }
}
