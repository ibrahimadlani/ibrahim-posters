//! Metric names, descriptions and the Prometheus exporter.
//!
//! Names are declared as constants and described once at startup rather than
//! being written inline at each call site. A metric emitted under a typo'd
//! name is not an error at compile time or at runtime — it simply appears as a
//! new series that nothing queries, which is exactly the failure mode a
//! dashboard cannot show you.

use std::time::Duration;

use metrics::{describe_counter, describe_gauge, describe_histogram, Unit};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// HTTP requests, labelled by endpoint and status class.
pub const REQUESTS: &str = "poster_requests_total";
/// End-to-end handler duration, labelled by endpoint.
pub const REQUEST_DURATION: &str = "poster_request_duration_seconds";
/// Cache lookups, labelled by tier and outcome.
pub const CACHE_LOOKUPS: &str = "poster_cache_lookups_total";
/// Time spent rendering, excluding fetch and storage.
pub const RENDER_DURATION: &str = "poster_render_duration_seconds";
/// Time spent fetching artwork from upstream, labelled by asset.
///
/// Labelled because background and logo have different sizes and different
/// cost profiles, and a single series would average them into a number that
/// describes neither.
pub const UPSTREAM_DURATION: &str = "poster_upstream_duration_seconds";
/// Requests rejected because no render slot became free in time.
pub const ADMISSION_REJECTED: &str = "poster_admission_rejected_total";
/// Time spent waiting for a render slot.
pub const ADMISSION_WAIT: &str = "poster_admission_wait_seconds";
/// Renders avoided because another request was already rendering the key.
pub const SINGLEFLIGHT_COLLAPSED: &str = "poster_singleflight_collapsed_total";
/// Render slots currently free.
pub const SLOTS_AVAILABLE: &str = "poster_render_slots_available";
/// Keys currently being rendered.
pub const INFLIGHT_KEYS: &str = "poster_inflight_keys";

/// Buckets for latency histograms, in seconds.
///
/// Concentrated between 10 ms and 500 ms, which is where the targets live: a
/// p50 of 80 ms and a p99 of 250 ms are both inside this range with buckets
/// either side, so neither is read off the edge of a histogram. The tail out
/// to 10 s exists to make a pathological request visible rather than to
/// measure it precisely.
const LATENCY_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.08, 0.1, 0.15, 0.25, 0.4, 0.6, 1.0, 2.5, 10.0,
];

/// Installs the Prometheus recorder and returns its render handle.
///
/// # Errors
///
/// Returns the builder error if a recorder is already installed, which happens
/// only if this is called twice in one process.
pub fn install() -> Result<PrometheusHandle, metrics_exporter_prometheus::BuildError> {
    let handle = PrometheusBuilder::new()
        .set_buckets(LATENCY_BUCKETS)?
        .install_recorder()?;
    describe();
    Ok(handle)
}

/// Registers a description for every metric this service emits.
///
/// Separate from [`install`] so tests can describe metrics without installing
/// a global recorder, which can only happen once per process.
pub fn describe() {
    describe_counter!(REQUESTS, "HTTP requests, by endpoint and status class");
    describe_histogram!(
        REQUEST_DURATION,
        Unit::Seconds,
        "End-to-end handler duration, by endpoint"
    );
    describe_counter!(CACHE_LOOKUPS, "Cache lookups, by tier and outcome");
    describe_histogram!(
        RENDER_DURATION,
        Unit::Seconds,
        "Time spent rendering, excluding fetch and storage"
    );
    describe_histogram!(
        UPSTREAM_DURATION,
        Unit::Seconds,
        "Time spent fetching artwork from upstream"
    );
    describe_counter!(
        ADMISSION_REJECTED,
        "Requests rejected because no render slot became free in time"
    );
    describe_histogram!(
        ADMISSION_WAIT,
        Unit::Seconds,
        "Time spent waiting for a render slot"
    );
    describe_counter!(
        SINGLEFLIGHT_COLLAPSED,
        "Renders avoided because another request was already rendering the key"
    );
    describe_gauge!(SLOTS_AVAILABLE, "Render slots currently free");
    describe_gauge!(INFLIGHT_KEYS, "Keys currently being rendered");
}

/// Which cache tier a lookup hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Rendered poster.
    L2,
    /// Persisted specification.
    Spec,
}

impl Tier {
    /// Returns the label value for this tier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::L2 => "l2",
            Self::Spec => "spec",
        }
    }
}

/// Records a cache lookup.
pub fn record_cache_lookup(tier: Tier, hit: bool) {
    metrics::counter!(
        CACHE_LOOKUPS,
        "tier" => tier.as_str(),
        "result" => if hit { "hit" } else { "miss" },
    )
    .increment(1);
}

/// Records a completed request.
///
/// The status *class* is used as a label rather than the code itself. A label
/// with unbounded cardinality is the standard way to make a Prometheus server
/// fall over, and while status codes are bounded in principle, the class is
/// what a dashboard actually groups by.
pub fn record_request(endpoint: &'static str, status: u16, elapsed: Duration) {
    let class = match status {
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    };
    metrics::counter!(REQUESTS, "endpoint" => endpoint, "status" => class).increment(1);
    metrics::histogram!(REQUEST_DURATION, "endpoint" => endpoint).record(elapsed.as_secs_f64());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_metric_name_is_prometheus_compatible() {
        // An invalid name is accepted by the recorder and rejected by the
        // scrape, so it fails where nobody is watching.
        for name in [
            REQUESTS,
            REQUEST_DURATION,
            CACHE_LOOKUPS,
            RENDER_DURATION,
            UPSTREAM_DURATION,
            ADMISSION_REJECTED,
            ADMISSION_WAIT,
            SINGLEFLIGHT_COLLAPSED,
            SLOTS_AVAILABLE,
            INFLIGHT_KEYS,
        ] {
            assert!(
                name.starts_with("poster_"),
                "{name} is missing the service prefix"
            );
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{name} contains a character Prometheus will reject"
            );
        }
    }

    #[test]
    fn counter_names_end_in_total_and_duration_names_in_seconds() {
        // Prometheus naming conventions, which is what makes rate() and
        // histogram_quantile() read correctly in a query.
        for name in [
            REQUESTS,
            CACHE_LOOKUPS,
            ADMISSION_REJECTED,
            SINGLEFLIGHT_COLLAPSED,
        ] {
            assert!(
                name.ends_with("_total"),
                "{name} is a counter without _total"
            );
        }
        for name in [
            REQUEST_DURATION,
            RENDER_DURATION,
            UPSTREAM_DURATION,
            ADMISSION_WAIT,
        ] {
            assert!(
                name.ends_with("_seconds"),
                "{name} is a duration without _seconds"
            );
        }
    }

    #[test]
    fn every_metric_name_is_distinct() {
        let mut names = vec![
            REQUESTS,
            REQUEST_DURATION,
            CACHE_LOOKUPS,
            RENDER_DURATION,
            UPSTREAM_DURATION,
            ADMISSION_REJECTED,
            ADMISSION_WAIT,
            SINGLEFLIGHT_COLLAPSED,
            SLOTS_AVAILABLE,
            INFLIGHT_KEYS,
        ];
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "two metrics share a name");
    }

    #[test]
    fn the_latency_buckets_bracket_both_targets() {
        // A target sitting above the largest bucket below it cannot be read
        // off the histogram at all.
        for target in [0.08_f64, 0.25] {
            assert!(
                LATENCY_BUCKETS.iter().any(|&b| b < target)
                    && LATENCY_BUCKETS.iter().any(|&b| b > target),
                "{target}s is not bracketed by buckets"
            );
        }
    }

    #[test]
    fn the_latency_buckets_are_ascending() {
        for pair in LATENCY_BUCKETS.windows(2) {
            assert!(pair[0] < pair[1], "buckets are not ordered: {pair:?}");
        }
    }

    #[test]
    fn tier_labels_are_distinct() {
        let mut labels: Vec<_> = [Tier::L2, Tier::Spec]
            .iter()
            .map(|tier| tier.as_str())
            .collect();
        let total = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), total);
    }
}
