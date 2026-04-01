/// Wall-clock performance guard for trace replay.
/// Run with: cargo test --release -p qc-simulate -- --ignored
use std::time::Instant;

use qc_simulate::baselines::LruPolicy;
use qc_simulate::engine::TraceReplayEngine;
use qc_simulate::synthetic::{self, SyntheticConfig};

#[test]
#[ignore]
fn lru_replay_1m_under_10_seconds() {
    let config = SyntheticConfig {
        num_objects: 1_000,
        num_requests: 1_000_000,
        seed: 42,
        ..SyntheticConfig::default()
    };
    let events = synthetic::generate(&config).unwrap();

    let start = Instant::now();
    let mut policy = LruPolicy::new(50_000_000);
    let _ = TraceReplayEngine::replay(&events, &mut policy).unwrap();
    let elapsed = start.elapsed();

    eprintln!("lru_replay_1m: {:?}", elapsed);
    assert!(
        elapsed.as_secs() < 10,
        "lru_replay_1m took {:?}, exceeds 10s target",
        elapsed
    );
}
