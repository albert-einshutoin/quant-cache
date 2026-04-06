/// Fast (non-ignored) comparison tests for CI.
/// Uses small traces (~1000 events) to verify relative policy ordering.
use qc_simulate::baselines::{BeladyPolicy, LruPolicy, S3FifoPolicy, SievePolicy};
use qc_simulate::comparator::Comparator;
use qc_simulate::engine::CachePolicy;
use qc_simulate::synthetic::{self, SyntheticConfig};

fn small_trace() -> Vec<qc_model::trace::RequestTraceEvent> {
    let config = SyntheticConfig {
        num_objects: 100,
        num_requests: 5_000,
        zipf_alpha: 0.8,
        seed: 42,
        ..SyntheticConfig::default()
    };
    synthetic::generate(&config).unwrap()
}

#[test]
fn belady_hit_ratio_is_highest() {
    let events = small_trace();
    let capacity = 50_000; // fits ~50 objects

    let mut lru = LruPolicy::new(capacity);
    let mut sieve = SievePolicy::new(capacity);
    let mut s3fifo = S3FifoPolicy::new(capacity);
    let mut belady = BeladyPolicy::new(&events, capacity);

    let report = Comparator::compare(
        &events,
        &mut [
            &mut lru as &mut dyn CachePolicy,
            &mut sieve,
            &mut s3fifo,
            &mut belady,
        ],
    )
    .unwrap();

    let belady_hr = report.results[3].metrics.hit_ratio;
    for r in &report.results[..3] {
        assert!(
            belady_hr >= r.metrics.hit_ratio - 1e-9,
            "Belady ({:.2}%) should have highest hit ratio, but {} got {:.2}%",
            belady_hr * 100.0,
            r.name,
            r.metrics.hit_ratio * 100.0
        );
    }
}

#[test]
fn all_policies_produce_positive_hits() {
    let events = small_trace();
    let capacity = 50_000;

    let mut lru = LruPolicy::new(capacity);
    let mut sieve = SievePolicy::new(capacity);
    let mut s3fifo = S3FifoPolicy::new(capacity);

    let report = Comparator::compare(
        &events,
        &mut [&mut lru as &mut dyn CachePolicy, &mut sieve, &mut s3fifo],
    )
    .unwrap();

    for r in &report.results {
        assert!(
            r.metrics.hit_ratio > 0.0,
            "{} should have positive hit ratio",
            r.name
        );
        assert!(
            r.metrics.estimated_cost_savings > 0.0,
            "{} should have positive cost savings",
            r.name
        );
    }
}
