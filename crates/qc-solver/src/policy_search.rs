use std::time::Instant;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use qc_model::object::ScoredObject;
use qc_model::policy_ir::{
    AdmissionRule, Backend, BypassRule, CacheKeyRule, PolicyIR, TtlClassRule,
};

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
    pub top_k: usize,
    /// Content types observed in the trace (for TTL class rule generation).
    pub content_types: Vec<String>,
}

impl Default for PolicySearchConfig {
    fn default() -> Self {
        Self {
            capacity_bytes: 10_737_418_240,
            max_iterations: 200,
            seed: 42,
            top_k: 5,
            content_types: Vec::new(),
        }
    }
}

/// Search the PolicyIR space for the best configuration.
///
/// Explores: backend × admission × bypass (including composite) × prewarm × TTL class rules.
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

    // Score percentiles
    let mut scores: Vec<f64> = scored.iter().map(|s| s.net_benefit).collect();
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p25 = scores.get(scores.len() / 4).copied().unwrap_or(0.0);
    let p50 = scores.get(scores.len() / 2).copied().unwrap_or(0.0);
    let p75 = scores.get(scores.len() * 3 / 4).copied().unwrap_or(0.0);

    // Size percentiles
    let mut sizes: Vec<u64> = scored.iter().map(|s| s.size_bytes).collect();
    sizes.sort();
    let size_p90 = sizes.get(sizes.len() * 9 / 10).copied().unwrap_or(0);
    let size_p95 = sizes.get(sizes.len() * 19 / 20).copied().unwrap_or(0);
    let median_size = sizes.get(sizes.len() / 2).copied().unwrap_or(1).max(1);

    // TTL candidates for class rules
    let ttl_options = [300u64, 600, 1800, 3600, 7200, 86400];

    // Precompute content type prefixes from observed types
    let ct_prefixes: Vec<String> = {
        let mut prefixes: Vec<String> = config
            .content_types
            .iter()
            .filter_map(|ct| ct.split('/').next().map(|p| format!("{p}/")))
            .collect();
        prefixes.sort();
        prefixes.dedup();
        prefixes
    };

    let backends = [Backend::Sieve, Backend::S3Fifo];
    let admission_thresholds = [0.0, p25 * 0.5, p25, p50, p75];

    let mut all_results: Vec<(PolicyIR, f64)> = Vec::new();
    let mut evaluated = 0;

    // Phase 1: Grid search (backend × admission × simple bypass)
    let bypass_sizes = [0u64, size_p95, size_p90];
    for &backend in &backends {
        for &adm_threshold in &admission_thresholds {
            for &bypass_size in &bypass_sizes {
                if evaluated >= config.max_iterations / 2 {
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

    // Phase 2: Random perturbation (all dimensions including TTL + composite bypass)
    let prewarm_counts = [0, 5, 10, 20];
    let freshness_thresholds = [1.0, 0.5, 0.3];

    let remaining = config.max_iterations.saturating_sub(evaluated);
    for _ in 0..remaining {
        let backend = backends[rng.gen_range(0..backends.len())];
        let adm_threshold = admission_thresholds[rng.gen_range(0..admission_thresholds.len())];
        let pw_count = prewarm_counts[rng.gen_range(0..prewarm_counts.len())];

        let admission_rule = if adm_threshold <= 0.0 {
            AdmissionRule::Always
        } else if rng.gen_bool(0.5) {
            AdmissionRule::ScoreThreshold {
                threshold: adm_threshold,
            }
        } else {
            AdmissionRule::ScoreDensityThreshold {
                threshold: adm_threshold / median_size as f64,
            }
        };

        // Bypass: None, SizeLimit, FreshnessRisk, or composite Any
        let bypass_rule = match rng.gen_range(0..4) {
            0 => BypassRule::None,
            1 => {
                let sz = bypass_sizes[rng.gen_range(0..bypass_sizes.len())];
                if sz == 0 {
                    BypassRule::None
                } else {
                    BypassRule::SizeLimit { max_bytes: sz }
                }
            }
            2 => {
                let ft = freshness_thresholds[rng.gen_range(0..freshness_thresholds.len())];
                if ft >= 1.0 {
                    BypassRule::None
                } else {
                    BypassRule::FreshnessRisk { threshold: ft }
                }
            }
            _ => {
                // Composite: SizeLimit + FreshnessRisk
                let sz = bypass_sizes[rng.gen_range(1..bypass_sizes.len())];
                let ft = freshness_thresholds[rng.gen_range(1..freshness_thresholds.len())];
                BypassRule::Any {
                    rules: vec![
                        BypassRule::SizeLimit { max_bytes: sz },
                        BypassRule::FreshnessRisk { threshold: ft },
                    ],
                }
            }
        };

        // Prewarm
        let prewarm_set = if pw_count > 0 {
            let mut by_score: Vec<&ScoredObject> = scored.iter().collect();
            by_score.sort_by(|a, b| {
                b.net_benefit
                    .partial_cmp(&a.net_benefit)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            by_score
                .iter()
                .take(pw_count)
                .map(|s| s.cache_key.clone())
                .collect()
        } else {
            vec![]
        };

        // TTL class rules: randomly assign TTLs to observed content-type prefixes
        let ttl_class_rules = if !ct_prefixes.is_empty() && rng.gen_bool(0.4) {
            ct_prefixes
                .iter()
                .map(|prefix| TtlClassRule {
                    content_type_pattern: prefix.clone(),
                    ttl_seconds: ttl_options[rng.gen_range(0..ttl_options.len())],
                })
                .collect()
        } else {
            vec![]
        };

        // Cache key rules: common normalization patterns
        let key_rule_candidates: Vec<Vec<CacheKeyRule>> = vec![
            vec![], // no rules
            vec![CacheKeyRule {
                pattern: r"[?&]utm_[^&]*".to_string(),
                replacement: "".to_string(),
            }],
            vec![CacheKeyRule {
                pattern: r"[?&]fbclid=[^&]*".to_string(),
                replacement: "".to_string(),
            }],
            vec![
                CacheKeyRule {
                    pattern: r"[?&]utm_[^&]*".to_string(),
                    replacement: "".to_string(),
                },
                CacheKeyRule {
                    pattern: r"[?&]fbclid=[^&]*".to_string(),
                    replacement: "".to_string(),
                },
            ],
        ];
        let cache_key_rules =
            key_rule_candidates[rng.gen_range(0..key_rule_candidates.len())].clone();

        let ir = PolicyIR {
            backend,
            capacity_bytes: config.capacity_bytes,
            admission_rule,
            bypass_rule,
            prewarm_set,
            ttl_class_rules,
            cache_key_rules,
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

/// Search method selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMethod {
    /// Grid + random perturbation (default).
    GridRandom,
    /// Simulated annealing over the PolicyIR configuration space.
    SimulatedAnnealing,
}

/// SA-based search over the PolicyIR configuration space.
///
/// Treats the entire PolicyIR as a state vector and applies single-dimension
/// mutations with Metropolis acceptance criterion.
pub fn search_sa<F>(
    config: &PolicySearchConfig,
    scored: &[ScoredObject],
    eval_fn: F,
) -> Result<PolicySearchResult, SolverError>
where
    F: Fn(&PolicyIR) -> Result<f64, SolverError>,
{
    let start = Instant::now();
    let mut rng = StdRng::seed_from_u64(config.seed);

    // Score/size percentiles for mutation ranges
    let mut scores: Vec<f64> = scored.iter().map(|s| s.net_benefit).collect();
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let score_percentiles = [
        0.0,
        scores.get(scores.len() / 4).copied().unwrap_or(0.0) * 0.5,
        scores.get(scores.len() / 4).copied().unwrap_or(0.0),
        scores.get(scores.len() / 2).copied().unwrap_or(0.0),
        scores.get(scores.len() * 3 / 4).copied().unwrap_or(0.0),
    ];

    let mut sizes: Vec<u64> = scored.iter().map(|s| s.size_bytes).collect();
    sizes.sort();
    let size_percentiles = [
        0u64,
        sizes.get(sizes.len() * 9 / 10).copied().unwrap_or(0),
        sizes.get(sizes.len() * 19 / 20).copied().unwrap_or(0),
    ];

    let ct_prefixes: Vec<String> = {
        let mut p: Vec<String> = config
            .content_types
            .iter()
            .filter_map(|ct| ct.split('/').next().map(|x| format!("{x}/")))
            .collect();
        p.sort();
        p.dedup();
        p
    };

    let ttl_options = [300u64, 600, 1800, 3600, 7200, 86400];
    let prewarm_counts = [0usize, 5, 10, 20];
    let median_size = sizes.get(sizes.len() / 2).copied().unwrap_or(1).max(1);

    let num_restarts = 3;
    let iters_per_restart = config.max_iterations / num_restarts.max(1);

    let mut best = PolicyIR {
        backend: Backend::Sieve,
        capacity_bytes: config.capacity_bytes,
        admission_rule: AdmissionRule::Always,
        bypass_rule: BypassRule::None,
        prewarm_set: vec![],
        ttl_class_rules: vec![],
        cache_key_rules: vec![],
    };
    let mut best_obj = f64::NEG_INFINITY;
    let mut all_results: Vec<(PolicyIR, f64)> = Vec::new();
    let mut evaluated = 0;

    let initial_backends = [Backend::Sieve, Backend::S3Fifo, Backend::Sieve];
    let initial_admissions = [
        AdmissionRule::Always,
        AdmissionRule::ScoreThreshold {
            threshold: score_percentiles.get(2).copied().unwrap_or(0.0),
        },
        AdmissionRule::ScoreDensityThreshold {
            threshold: score_percentiles.get(3).copied().unwrap_or(0.0) / median_size as f64,
        },
    ];

    for restart in 0..num_restarts {
        let mut current = PolicyIR {
            backend: initial_backends[restart % initial_backends.len()],
            capacity_bytes: config.capacity_bytes,
            admission_rule: initial_admissions[restart % initial_admissions.len()].clone(),
            bypass_rule: BypassRule::None,
            prewarm_set: vec![],
            ttl_class_rules: vec![],
            cache_key_rules: vec![],
        };

        let mut current_obj = eval_fn(&current).unwrap_or(f64::NEG_INFINITY);
        all_results.push((current.clone(), current_obj));
        evaluated += 1;

        if current_obj > best_obj {
            best = current.clone();
            best_obj = current_obj;
        }

        let initial_temp = 10.0;
        let cooling_rate = (1.0 - (3.0 / iters_per_restart.max(10) as f64)).max(0.9);
        let mut temp = initial_temp;

        for _ in 0..iters_per_restart {
            // Mutate one random dimension (6 dimensions, capacity excluded)
            let mut candidate = current.clone();
            let dimension = rng.gen_range(0..6);

            match dimension {
                0 => {
                    // Backend
                    candidate.backend = if candidate.backend == Backend::Sieve {
                        Backend::S3Fifo
                    } else {
                        Backend::Sieve
                    };
                }
                1 => {
                    // Admission rule
                    let variants = [0, 1, 2];
                    match variants[rng.gen_range(0..3)] {
                        0 => candidate.admission_rule = AdmissionRule::Always,
                        1 => {
                            let t = score_percentiles[rng.gen_range(0..score_percentiles.len())];
                            candidate.admission_rule =
                                AdmissionRule::ScoreThreshold { threshold: t };
                        }
                        _ => {
                            let t = score_percentiles[rng.gen_range(0..score_percentiles.len())]
                                / median_size as f64;
                            candidate.admission_rule =
                                AdmissionRule::ScoreDensityThreshold { threshold: t };
                        }
                    }
                }
                2 => {
                    // Bypass rule
                    match rng.gen_range(0..4) {
                        0 => candidate.bypass_rule = BypassRule::None,
                        1 => {
                            let sz = size_percentiles[rng.gen_range(0..size_percentiles.len())];
                            candidate.bypass_rule = if sz == 0 {
                                BypassRule::None
                            } else {
                                BypassRule::SizeLimit { max_bytes: sz }
                            };
                        }
                        2 => {
                            let ft = [1.0, 0.5, 0.3][rng.gen_range(0..3)];
                            candidate.bypass_rule = if ft >= 1.0 {
                                BypassRule::None
                            } else {
                                BypassRule::FreshnessRisk { threshold: ft }
                            };
                        }
                        _ => {
                            let sz =
                                size_percentiles[rng.gen_range(1..size_percentiles.len()).max(1)];
                            let ft = [0.5, 0.3][rng.gen_range(0..2)];
                            candidate.bypass_rule = BypassRule::Any {
                                rules: vec![
                                    BypassRule::SizeLimit {
                                        max_bytes: sz.max(1),
                                    },
                                    BypassRule::FreshnessRisk { threshold: ft },
                                ],
                            };
                        }
                    }
                }
                3 => {
                    // Prewarm
                    let pw = prewarm_counts[rng.gen_range(0..prewarm_counts.len())];
                    if pw > 0 {
                        let mut by_score: Vec<&ScoredObject> = scored.iter().collect();
                        by_score.sort_by(|a, b| {
                            b.net_benefit
                                .partial_cmp(&a.net_benefit)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        candidate.prewarm_set = by_score
                            .iter()
                            .take(pw)
                            .map(|s| s.cache_key.clone())
                            .collect();
                    } else {
                        candidate.prewarm_set = vec![];
                    }
                }
                4 => {
                    // TTL class rules
                    if !ct_prefixes.is_empty() && rng.gen_bool(0.6) {
                        candidate.ttl_class_rules = ct_prefixes
                            .iter()
                            .map(|p| TtlClassRule {
                                content_type_pattern: p.clone(),
                                ttl_seconds: ttl_options[rng.gen_range(0..ttl_options.len())],
                            })
                            .collect();
                    } else {
                        candidate.ttl_class_rules = vec![];
                    }
                }
                _ => {
                    // Cache key rules
                    let key_options: Vec<Vec<CacheKeyRule>> = vec![
                        vec![],
                        vec![CacheKeyRule {
                            pattern: r"[?&]utm_[^&]*".into(),
                            replacement: "".into(),
                        }],
                        vec![
                            CacheKeyRule {
                                pattern: r"[?&]utm_[^&]*".into(),
                                replacement: "".into(),
                            },
                            CacheKeyRule {
                                pattern: r"[?&]fbclid=[^&]*".into(),
                                replacement: "".into(),
                            },
                        ],
                    ];
                    candidate.cache_key_rules =
                        key_options[rng.gen_range(0..key_options.len())].clone();
                }
            }

            // Evaluate candidate
            if let Ok(obj) = eval_fn(&candidate) {
                let delta = obj - current_obj;
                let accept = if delta > 0.0 {
                    true
                } else {
                    temp > 0.0 && rng.gen::<f64>() < (delta / temp).exp()
                };

                if accept {
                    current = candidate.clone();
                    current_obj = obj;

                    if obj > best_obj {
                        best = candidate.clone();
                        best_obj = obj;
                    }
                }

                all_results.push((candidate, obj));
            }
            evaluated += 1;
            temp *= cooling_rate;
        }
    } // end restart loop

    all_results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let top_candidates: Vec<(PolicyIR, f64)> =
        all_results.iter().take(config.top_k).cloned().collect();

    Ok(PolicySearchResult {
        best_ir: best,
        best_objective: best_obj,
        candidates_evaluated: evaluated,
        search_time_ms: start.elapsed().as_millis() as u64,
        top_candidates,
    })
}
