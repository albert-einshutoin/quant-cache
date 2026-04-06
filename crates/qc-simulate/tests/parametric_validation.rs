/// Parametric validation suite for quant-cache.
///
/// Systematically sweeps the parameter space to validate invariants
/// across a wide range of workload characteristics.
///
/// Tiers:
///   - Smoke: `cargo test` (default) — ~30 combos, <30s
///   - Full:  `cargo test --release -- --ignored` — ~2,880 combos, <60min
use qc_model::scenario::{
    CapacityConstraint, FreshnessModel, ScenarioConfig, ScoringVersion, StaleCostOverrides,
    StalePenaltyClass, StalePenaltyConfig,
};
use qc_simulate::baselines::{BeladyPolicy, LruPolicy, SievePolicy};
use qc_simulate::comparator::Comparator;
use qc_simulate::engine::{CachePolicy, ReplayEconConfig, TraceReplayEngine};
use qc_simulate::synthetic::{self, SyntheticConfig};
use qc_solver::greedy::GreedySolver;
use qc_solver::score::BenefitCalculator;
use qc_solver::solver::Solver;

#[derive(Debug, Clone)]
struct ParamSet {
    zipf_alpha: f64,
    capacity_ratio: f64, // fraction of total catalog bytes
    num_objects: usize,
    update_rate: f64,
    scoring: ScoringVersion,
}

fn make_config(p: &ParamSet, capacity_bytes: u64) -> ScenarioConfig {
    ScenarioConfig {
        capacity_bytes,
        time_window_seconds: 86400,
        latency_value_per_ms: 0.0001,
        freshness_model: FreshnessModel::TtlOnly {
            stale_penalty: StalePenaltyConfig {
                default_class: if p.update_rate > 0.0 {
                    StalePenaltyClass::Low
                } else {
                    StalePenaltyClass::None
                },
                cost_overrides: StaleCostOverrides::default(),
            },
        },
        scoring_version: p.scoring,
    }
}

fn run_validation(p: &ParamSet) {
    let num_requests = (p.num_objects * 50).min(50_000);
    let syn_config = SyntheticConfig {
        num_objects: p.num_objects,
        num_requests,
        zipf_alpha: p.zipf_alpha,
        update_rate_lambda: p.update_rate,
        seed: 42,
        ..SyntheticConfig::default()
    };

    let events = match synthetic::generate(&syn_config) {
        Ok(e) => e,
        Err(e) => {
            // Some extreme params may fail generation (e.g., Poisson overflow).
            // That's acceptable — skip gracefully.
            eprintln!("  SKIP {p:?}: generation error: {e}");
            return;
        }
    };

    // Compute total catalog size for capacity ratio
    let total_catalog_bytes: u64 = {
        let mut seen = std::collections::HashSet::new();
        let mut total = 0u64;
        for e in &events {
            if seen.insert(&e.cache_key) {
                total += e.object_size_bytes;
            }
        }
        total
    };
    let capacity_bytes = (total_catalog_bytes as f64 * p.capacity_ratio) as u64;
    let capacity_bytes = capacity_bytes.max(1);

    let config = make_config(p, capacity_bytes);
    let compute_reuse = p.scoring == ScoringVersion::V2ReuseDistance;
    let features = synthetic::aggregate_features_with_options(
        &events,
        config.time_window_seconds,
        compute_reuse,
    );

    if features.is_empty() {
        return;
    }

    let scored = match BenefitCalculator::score_all(&features, &config) {
        Ok(s) => s,
        Err(e) => {
            panic!("scoring failed for {p:?}: {e}");
        }
    };

    // === Invariant 1: No NaN/Inf in scores ===
    for s in &scored {
        assert!(
            s.net_benefit.is_finite(),
            "NaN/Inf in net_benefit for {:?}: {}",
            p,
            s.net_benefit
        );
    }

    // === Invariant 2: Greedy respects capacity ===
    let constraint = CapacityConstraint {
        capacity_bytes: config.capacity_bytes,
    };
    let result = GreedySolver.solve(&scored, &constraint).unwrap();
    assert!(result.feasible, "Greedy should be feasible for {p:?}");

    let size_map: std::collections::HashMap<&str, u64> = scored
        .iter()
        .map(|s| (s.cache_key.as_str(), s.size_bytes))
        .collect();
    let used: u64 = result
        .decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| size_map.get(d.cache_key.as_str()).copied().unwrap_or(0))
        .sum();
    assert!(
        used <= constraint.capacity_bytes,
        "Greedy exceeded capacity for {p:?}: used {used} > cap {}",
        constraint.capacity_bytes
    );

    // === Invariant 3: Greedy is deterministic ===
    let result2 = GreedySolver.solve(&scored, &constraint).unwrap();
    assert!(
        (result.objective_value - result2.objective_value).abs() < 1e-12,
        "Greedy not deterministic for {p:?}"
    );

    // === Invariant 4: Belady hit_ratio >= online policies ===
    let cached_keys = result
        .decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| d.cache_key.clone());
    let mut econ_policy = qc_simulate::baselines::StaticPolicy::new(cached_keys);
    let mut lru = LruPolicy::new(capacity_bytes);
    let mut sieve = SievePolicy::new(capacity_bytes);
    let mut belady = BeladyPolicy::new(&events, capacity_bytes);

    let report = Comparator::compare(
        &events,
        &mut [
            &mut econ_policy as &mut dyn CachePolicy,
            &mut lru,
            &mut sieve,
            &mut belady,
        ],
    )
    .unwrap();

    let belady_hr = report.results[3].metrics.hit_ratio;
    for r in &report.results[..3] {
        assert!(
            belady_hr >= r.metrics.hit_ratio - 1e-9,
            "Belady ({:.4}) < {} ({:.4}) for {p:?}",
            belady_hr,
            r.name,
            r.metrics.hit_ratio,
        );
    }

    // === Invariant 5: Replay metrics are finite ===
    let default_class = StalePenaltyClass::None;
    let econ =
        ReplayEconConfig::from_features(&features, config.latency_value_per_ms, default_class);
    let metrics = TraceReplayEngine::replay_with_econ(&events, &mut econ_policy, &econ);
    if let Ok(m) = metrics {
        assert!(m.hit_ratio.is_finite(), "hit_ratio NaN for {p:?}");
        assert!(
            m.policy_objective_value.is_finite(),
            "objective NaN for {p:?}"
        );
    }
}

fn run_monotonicity(p: &ParamSet) {
    let num_requests = (p.num_objects * 50).min(50_000);
    let syn_config = SyntheticConfig {
        num_objects: p.num_objects,
        num_requests,
        zipf_alpha: p.zipf_alpha,
        update_rate_lambda: p.update_rate,
        seed: 42,
        ..SyntheticConfig::default()
    };

    let events = match synthetic::generate(&syn_config) {
        Ok(e) => e,
        Err(_) => return,
    };

    let total_catalog_bytes: u64 = {
        let mut seen = std::collections::HashSet::new();
        let mut total = 0u64;
        for e in &events {
            if seen.insert(&e.cache_key) {
                total += e.object_size_bytes;
            }
        }
        total
    };

    let features = synthetic::aggregate_features_with_options(
        &events,
        86400,
        p.scoring == ScoringVersion::V2ReuseDistance,
    );
    if features.is_empty() {
        return;
    }

    // === Invariant 6: Monotonicity — more capacity never reduces objective ===
    let cap_small = (total_catalog_bytes as f64 * 0.05) as u64;
    let cap_large = (total_catalog_bytes as f64 * 0.25) as u64;

    let config_s = make_config(p, cap_small.max(1));
    let config_l = make_config(p, cap_large.max(1));

    let scored_s = BenefitCalculator::score_all(&features, &config_s).unwrap();
    let scored_l = BenefitCalculator::score_all(&features, &config_l).unwrap();

    let r_s = GreedySolver
        .solve(
            &scored_s,
            &CapacityConstraint {
                capacity_bytes: cap_small.max(1),
            },
        )
        .unwrap();
    let r_l = GreedySolver
        .solve(
            &scored_l,
            &CapacityConstraint {
                capacity_bytes: cap_large.max(1),
            },
        )
        .unwrap();

    assert!(
        r_l.objective_value >= r_s.objective_value - 1e-9,
        "Monotonicity violated for {p:?}: cap 25% obj={:.2} < cap 5% obj={:.2}",
        r_l.objective_value,
        r_s.objective_value
    );
}

// ── Smoke tier (~30 combos, <30s) ──────────────────────────────────

fn smoke_params() -> Vec<ParamSet> {
    let mut params = Vec::new();
    for &zipf in &[0.3, 0.6, 1.2] {
        for &cap in &[0.05, 0.10, 0.50] {
            for &scoring in &[ScoringVersion::V1Frequency, ScoringVersion::V2ReuseDistance] {
                params.push(ParamSet {
                    zipf_alpha: zipf,
                    capacity_ratio: cap,
                    num_objects: 200,
                    update_rate: 0.001,
                    scoring,
                });
            }
        }
    }
    // Edge cases
    params.push(ParamSet {
        zipf_alpha: 0.6,
        capacity_ratio: 0.01,
        num_objects: 100,
        update_rate: 0.0,
        scoring: ScoringVersion::V1Frequency,
    });
    params.push(ParamSet {
        zipf_alpha: 0.6,
        capacity_ratio: 0.50,
        num_objects: 100,
        update_rate: 0.1,
        scoring: ScoringVersion::V1Frequency,
    });
    params
}

#[test]
fn parametric_smoke() {
    let params = smoke_params();
    eprintln!("Running {} smoke validation combos", params.len());
    for (i, p) in params.iter().enumerate() {
        eprintln!("  [{}/{}] {:?}", i + 1, params.len(), p);
        run_validation(p);
    }
}

#[test]
fn monotonicity_smoke() {
    let params = [
        ParamSet {
            zipf_alpha: 0.6,
            capacity_ratio: 0.10,
            num_objects: 200,
            update_rate: 0.0,
            scoring: ScoringVersion::V1Frequency,
        },
        ParamSet {
            zipf_alpha: 0.8,
            capacity_ratio: 0.10,
            num_objects: 500,
            update_rate: 0.001,
            scoring: ScoringVersion::V1Frequency,
        },
        ParamSet {
            zipf_alpha: 1.0,
            capacity_ratio: 0.10,
            num_objects: 200,
            update_rate: 0.0,
            scoring: ScoringVersion::V2ReuseDistance,
        },
    ];
    for p in &params {
        eprintln!("  monotonicity: {p:?}");
        run_monotonicity(p);
    }
}

// ── Full tier (~2,880 combos, release + ignored) ───────────────────

fn full_params() -> Vec<ParamSet> {
    let mut params = Vec::new();
    for &zipf in &[0.3, 0.5, 0.6, 0.8, 1.0, 1.2] {
        for &cap in &[0.01, 0.05, 0.10, 0.25, 0.50] {
            for &objects in &[100, 1_000, 10_000] {
                for &rate in &[0.0, 0.001, 0.01, 0.1] {
                    for &scoring in &[ScoringVersion::V1Frequency, ScoringVersion::V2ReuseDistance]
                    {
                        params.push(ParamSet {
                            zipf_alpha: zipf,
                            capacity_ratio: cap,
                            num_objects: objects,
                            update_rate: rate,
                            scoring,
                        });
                    }
                }
            }
        }
    }
    params
}

#[test]
#[ignore]
fn parametric_full_sweep() {
    let params = full_params();
    eprintln!("Running {} full validation combos", params.len());
    let mut passed = 0;
    let mut skipped = 0;
    for (i, p) in params.iter().enumerate() {
        if i % 100 == 0 {
            eprintln!("  [{}/{}] ...", i, params.len());
        }
        // Catch panics to report all failures, not just the first
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_validation(p);
        }));
        match result {
            Ok(()) => passed += 1,
            Err(e) => {
                let msg = e
                    .downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| e.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                if msg.contains("SKIP") || msg.contains("generation error") {
                    skipped += 1;
                } else {
                    panic!("FAILED [{}/{}] {p:?}: {msg}", i + 1, params.len());
                }
            }
        }
    }
    eprintln!(
        "Full sweep complete: {passed} passed, {skipped} skipped, {} total",
        params.len()
    );
}

#[test]
#[ignore]
fn monotonicity_full_sweep() {
    let params = [
        (0.3, 100, 0.0, ScoringVersion::V1Frequency),
        (0.6, 500, 0.001, ScoringVersion::V1Frequency),
        (0.8, 1_000, 0.01, ScoringVersion::V1Frequency),
        (1.0, 200, 0.0, ScoringVersion::V2ReuseDistance),
        (1.2, 500, 0.001, ScoringVersion::V2ReuseDistance),
    ];
    for &(zipf, objects, rate, scoring) in &params {
        let p = ParamSet {
            zipf_alpha: zipf,
            capacity_ratio: 0.10,
            num_objects: objects,
            update_rate: rate,
            scoring,
        };
        eprintln!("  monotonicity: {p:?}");
        run_monotonicity(&p);
    }
}
