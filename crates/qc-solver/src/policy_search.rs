use std::time::Instant;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use qc_model::object::ScoredObject;
use qc_model::policy_ir::{AdmissionRule, Backend, BypassRule, PolicyIR};

use crate::error::SolverError;

/// Result of policy search.
#[derive(Debug, Clone)]
pub struct PolicySearchResult {
    pub best_ir: PolicyIR,
    pub best_objective: f64,
    pub candidates_evaluated: usize,
    pub search_time_ms: u64,
    pub top_candidates: Vec<(PolicyIR, f64)>,
}

/// Search configuration.
#[derive(Debug, Clone)]
pub struct PolicySearchConfig {
    pub capacity_bytes: u64,
    pub max_iterations: usize,
    pub seed: u64,
    /// Number of top candidates to return.
    pub top_k: usize,
}

impl Default for PolicySearchConfig {
    fn default() -> Self {
        Self {
            capacity_bytes: 10_737_418_240,
            max_iterations: 200,
            seed: 42,
            top_k: 5,
        }
    }
}

/// Search the PolicyIR space for the best configuration.
///
/// Uses a structured sweep + random perturbation approach:
/// 1. Enumerate backend × admission threshold grid
/// 2. Random bypass and prewarm variations
/// 3. Evaluate each via IrPolicy replay
///
/// `eval_fn` takes a PolicyIR and returns its objective value.
pub fn search<F>(
    config: &PolicySearchConfig,
    scored: &[ScoredObject],
    eval_fn: F,
) -> Result<PolicySearchResult, SolverError>
where
    F: Fn(&PolicyIR) -> Result<f64, SolverError>,
{
    let start = Instant::now();
    let mut rng = StdRng::seed_from_u64(config.seed);

    // Compute score percentiles for threshold grid
    let mut scores: Vec<f64> = scored.iter().map(|s| s.net_benefit).collect();
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p25 = scores.get(scores.len() / 4).copied().unwrap_or(0.0);
    let p50 = scores.get(scores.len() / 2).copied().unwrap_or(0.0);
    let p75 = scores.get(scores.len() * 3 / 4).copied().unwrap_or(0.0);

    // Compute size percentiles for bypass grid
    let mut sizes: Vec<u64> = scored.iter().map(|s| s.size_bytes).collect();
    sizes.sort();
    let size_p90 = sizes.get(sizes.len() * 9 / 10).copied().unwrap_or(u64::MAX);
    let size_p95 = sizes
        .get(sizes.len() * 19 / 20)
        .copied()
        .unwrap_or(u64::MAX);

    let backends = [Backend::Sieve, Backend::S3Fifo];
    let admission_thresholds = [0.0, p25 * 0.5, p25, p50, p75];
    let bypass_sizes = [0u64, size_p95, size_p90];
    let freshness_thresholds = [1.0, 0.5, 0.3]; // 1.0 = no bypass
    let prewarm_counts = [0, 5, 10, 20];

    let mut all_results: Vec<(PolicyIR, f64)> = Vec::new();
    let mut evaluated = 0;

    // Structured grid search
    for &backend in &backends {
        for &adm_threshold in &admission_thresholds {
            for &bypass_size in &bypass_sizes {
                if evaluated >= config.max_iterations {
                    break;
                }

                let admission_rule = if adm_threshold <= 0.0 {
                    AdmissionRule::Always
                } else {
                    AdmissionRule::ScoreThreshold {
                        threshold: adm_threshold,
                    }
                };

                let bypass_rule = if bypass_size == 0 {
                    BypassRule::None
                } else {
                    BypassRule::SizeLimit {
                        max_bytes: bypass_size,
                    }
                };

                let ir = PolicyIR {
                    backend,
                    capacity_bytes: config.capacity_bytes,
                    admission_rule,
                    bypass_rule,
                    prewarm_set: vec![],
                    ttl_class_rules: vec![],
                    cache_key_rules: vec![],
                };

                if let Ok(obj) = eval_fn(&ir) {
                    all_results.push((ir, obj));
                }
                evaluated += 1;
            }
        }
    }

    // Random perturbation phase: try combinations with prewarm and freshness bypass
    let remaining = config.max_iterations.saturating_sub(evaluated);
    for _ in 0..remaining {
        let backend = backends[rng.gen_range(0..backends.len())];
        let adm_idx = rng.gen_range(0..admission_thresholds.len());
        let adm_threshold = admission_thresholds[adm_idx];
        let fresh_idx = rng.gen_range(0..freshness_thresholds.len());
        let pw_count = prewarm_counts[rng.gen_range(0..prewarm_counts.len())];

        let admission_rule = if adm_threshold <= 0.0 {
            AdmissionRule::Always
        } else {
            AdmissionRule::ScoreDensityThreshold {
                threshold: adm_threshold / sizes.get(sizes.len() / 2).copied().unwrap_or(1) as f64,
            }
        };

        let fresh_t = freshness_thresholds[fresh_idx];
        let bypass_rule = if fresh_t >= 1.0 {
            BypassRule::None
        } else {
            BypassRule::FreshnessRisk { threshold: fresh_t }
        };

        // Prewarm: top-k by score
        let mut prewarm_set = Vec::new();
        if pw_count > 0 {
            let mut by_score: Vec<&ScoredObject> = scored.iter().collect();
            by_score.sort_by(|a, b| {
                b.net_benefit
                    .partial_cmp(&a.net_benefit)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            prewarm_set = by_score
                .iter()
                .take(pw_count)
                .map(|s| s.cache_key.clone())
                .collect();
        }

        let ir = PolicyIR {
            backend,
            capacity_bytes: config.capacity_bytes,
            admission_rule,
            bypass_rule,
            prewarm_set,
            ttl_class_rules: vec![],
            cache_key_rules: vec![],
        };

        if let Ok(obj) = eval_fn(&ir) {
            all_results.push((ir, obj));
        }
        evaluated += 1;
    }

    // Sort by objective descending
    all_results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let best = all_results.first().ok_or(SolverError::Infeasible)?;

    let top_candidates: Vec<(PolicyIR, f64)> =
        all_results.iter().take(config.top_k).cloned().collect();

    Ok(PolicySearchResult {
        best_ir: best.0.clone(),
        best_objective: best.1,
        candidates_evaluated: evaluated,
        search_time_ms: start.elapsed().as_millis() as u64,
        top_candidates,
    })
}
