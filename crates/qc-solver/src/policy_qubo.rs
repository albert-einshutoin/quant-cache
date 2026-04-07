//! QUBO formulation for PolicyIR DSL search.
//!
//! Encodes PolicyIR configuration choices as binary variables and estimates
//! pairwise interaction weights from trace-driven evaluation. This allows
//! the QUBO solver (simulated annealing) to search the policy space with
//! awareness of how configuration dimensions interact.
//!
//! The search space is small (~288 states), so this is primarily a demonstration
//! of the QUBO-over-DSL concept. The real value is when interaction weights
//! reveal non-obvious synergies (e.g., S3-FIFO + aggressive admission filtering).

use qc_model::object::ScoredObject;
use qc_model::policy_ir::{AdmissionRule, Backend, BypassRule, PolicyIR};

use crate::error::SolverError;
use crate::policy_search::PolicySearchResult;
use crate::qubo::{
    PairwiseInteraction, QuadraticProblem, QuadraticSolver, SimulatedAnnealingSolver,
};
use crate::solver::SolverResult;

/// Context for DSL choice application (precomputed from scores).
#[derive(Debug, Clone)]
pub struct DslContext {
    pub capacity_bytes: u64,
    pub score_p25: f64,
    pub score_p50: f64,
    pub score_p75: f64,
    pub median_size: u64,
    pub size_p90: u64,
    pub size_p95: u64,
    pub prewarm_keys: Vec<String>,
}

impl DslContext {
    pub fn from_scored(scored: &[ScoredObject], capacity_bytes: u64) -> Self {
        let mut scores: Vec<f64> = scored.iter().map(|s| s.net_benefit).collect();
        scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut sizes: Vec<u64> = scored.iter().map(|s| s.size_bytes).collect();
        sizes.sort();

        let score_p25 = scores.get(scores.len() / 4).copied().unwrap_or(0.0);
        let score_p50 = scores.get(scores.len() / 2).copied().unwrap_or(0.0);
        let score_p75 = scores.get(scores.len() * 3 / 4).copied().unwrap_or(0.0);
        let median_size = sizes.get(sizes.len() / 2).copied().unwrap_or(1).max(1);
        let size_p90 = sizes.get(sizes.len() * 9 / 10).copied().unwrap_or(0);
        let size_p95 = sizes.get(sizes.len() * 19 / 20).copied().unwrap_or(0);

        let mut by_score: Vec<&ScoredObject> = scored.iter().collect();
        by_score.sort_by(|a, b| {
            b.net_benefit
                .partial_cmp(&a.net_benefit)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let prewarm_keys: Vec<String> = by_score
            .iter()
            .take(20)
            .map(|s| s.cache_key.clone())
            .collect();

        Self {
            capacity_bytes,
            score_p25,
            score_p50,
            score_p75,
            median_size,
            size_p90,
            size_p95,
            prewarm_keys,
        }
    }
}

/// Encode PolicyIR DSL choices as binary variables for QUBO.
///
/// Each variable represents a specific configuration choice.
/// The QUBO solver selects at most one choice per dimension group.
pub fn encode_dsl_variables(ctx: &DslContext) -> Vec<DslVariable> {
    let mut vars = Vec::new();

    // Dimension 0: Backend (2 choices)
    vars.push(DslVariable {
        dimension: "backend",
        label: "SIEVE".into(),
        apply: Box::new(|ir, _| ir.backend = Backend::Sieve),
    });
    vars.push(DslVariable {
        dimension: "backend",
        label: "S3FIFO".into(),
        apply: Box::new(|ir, _| ir.backend = Backend::S3Fifo),
    });

    // Dimension 1: Admission (4 choices)
    vars.push(DslVariable {
        dimension: "admission",
        label: "Always".into(),
        apply: Box::new(|ir, _| ir.admission_rule = AdmissionRule::Always),
    });
    let p25 = ctx.score_p25;
    vars.push(DslVariable {
        dimension: "admission",
        label: "ScoreT_p25".into(),
        apply: Box::new(move |ir, _| {
            ir.admission_rule = AdmissionRule::ScoreThreshold { threshold: p25 };
        }),
    });
    let p50 = ctx.score_p50;
    vars.push(DslVariable {
        dimension: "admission",
        label: "ScoreT_p50".into(),
        apply: Box::new(move |ir, _| {
            ir.admission_rule = AdmissionRule::ScoreThreshold { threshold: p50 };
        }),
    });
    let p75 = ctx.score_p75;
    let ms = ctx.median_size;
    vars.push(DslVariable {
        dimension: "admission",
        label: "DensityT_p75".into(),
        apply: Box::new(move |ir, _| {
            ir.admission_rule = AdmissionRule::ScoreDensityThreshold {
                threshold: p75 / ms as f64,
            };
        }),
    });

    // Dimension 2: Bypass (4 choices)
    vars.push(DslVariable {
        dimension: "bypass",
        label: "None".into(),
        apply: Box::new(|ir, _| ir.bypass_rule = BypassRule::None),
    });
    let sp95 = ctx.size_p95;
    vars.push(DslVariable {
        dimension: "bypass",
        label: "Size_p95".into(),
        apply: Box::new(move |ir, _| {
            ir.bypass_rule = BypassRule::SizeLimit { max_bytes: sp95 };
        }),
    });
    vars.push(DslVariable {
        dimension: "bypass",
        label: "Freshness_0.3".into(),
        apply: Box::new(|ir, _| {
            ir.bypass_rule = BypassRule::FreshnessRisk { threshold: 0.3 };
        }),
    });
    let sp90 = ctx.size_p90;
    vars.push(DslVariable {
        dimension: "bypass",
        label: "Composite".into(),
        apply: Box::new(move |ir, _| {
            ir.bypass_rule = BypassRule::Any {
                rules: vec![
                    BypassRule::SizeLimit {
                        max_bytes: sp90.max(1),
                    },
                    BypassRule::FreshnessRisk { threshold: 0.3 },
                ],
            };
        }),
    });

    // Dimension 3: Prewarm (3 choices)
    vars.push(DslVariable {
        dimension: "prewarm",
        label: "None".into(),
        apply: Box::new(|ir, _| ir.prewarm_set = vec![]),
    });
    vars.push(DslVariable {
        dimension: "prewarm",
        label: "Top10".into(),
        apply: Box::new(|ir, ctx| {
            ir.prewarm_set = ctx.prewarm_keys.iter().take(10).cloned().collect();
        }),
    });
    vars.push(DslVariable {
        dimension: "prewarm",
        label: "Top20".into(),
        apply: Box::new(|ir, ctx| {
            ir.prewarm_set = ctx.prewarm_keys.iter().take(20).cloned().collect();
        }),
    });

    vars
}

/// Apply function for a DSL variable.
type DslApplyFn = Box<dyn Fn(&mut PolicyIR, &DslContext) + Send + Sync>;

/// A binary variable in the QUBO encoding of PolicyIR DSL.
pub struct DslVariable {
    pub dimension: &'static str,
    pub label: String,
    pub apply: DslApplyFn,
}

impl std::fmt::Debug for DslVariable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DslVariable")
            .field("dimension", &self.dimension)
            .field("label", &self.label)
            .finish()
    }
}

/// Estimate interaction weights between DSL variables by evaluating
/// pairs of choices and measuring objective synergy.
///
/// For each pair (i, j) in different dimensions, the interaction weight is:
///   w_ij = obj(i+j) - obj(i) - obj(j) + obj(baseline)
///
/// This captures the synergy or conflict between two choices beyond their
/// individual contributions.
pub fn estimate_interactions<F>(
    vars: &[DslVariable],
    ctx: &DslContext,
    eval_fn: &F,
) -> Vec<PairwiseInteraction>
where
    F: Fn(&PolicyIR) -> Result<f64, SolverError>,
{
    let base_ir = PolicyIR {
        backend: Backend::Sieve,
        capacity_bytes: ctx.capacity_bytes,
        admission_rule: AdmissionRule::Always,
        bypass_rule: BypassRule::None,
        prewarm_set: vec![],
        ttl_class_rules: vec![],
        cache_key_rules: vec![],
    };
    let base_obj = eval_fn(&base_ir).unwrap_or(0.0);

    // Compute individual effects
    let individual: Vec<f64> = vars
        .iter()
        .map(|v| {
            let mut ir = base_ir.clone();
            (v.apply)(&mut ir, ctx);
            eval_fn(&ir).unwrap_or(0.0) - base_obj
        })
        .collect();

    let mut interactions = Vec::new();
    for i in 0..vars.len() {
        for j in (i + 1)..vars.len() {
            // Only compute cross-dimension interactions
            if vars[i].dimension == vars[j].dimension {
                continue;
            }
            let mut ir = base_ir.clone();
            (vars[i].apply)(&mut ir, ctx);
            (vars[j].apply)(&mut ir, ctx);
            let joint_obj = eval_fn(&ir).unwrap_or(0.0);

            let synergy = joint_obj - base_obj - individual[i] - individual[j];
            if synergy.abs() > 1e-9 {
                interactions.push(PairwiseInteraction {
                    i: i as u32,
                    j: j as u32,
                    weight: synergy,
                });
            }
        }
    }

    interactions
}

/// Run QUBO-based PolicyIR DSL search.
///
/// 1. Encode DSL choices as binary variables
/// 2. Estimate linear benefits and pairwise interactions from trace
/// 3. Solve with SA QUBO solver
/// 4. Decode winning variables back to PolicyIR
pub fn search_qubo<F>(
    scored: &[ScoredObject],
    capacity_bytes: u64,
    eval_fn: F,
) -> Result<PolicySearchResult, SolverError>
where
    F: Fn(&PolicyIR) -> Result<f64, SolverError>,
{
    let start = std::time::Instant::now();
    let ctx = DslContext::from_scored(scored, capacity_bytes);
    let vars = encode_dsl_variables(&ctx);

    let base_ir = PolicyIR {
        backend: Backend::Sieve,
        capacity_bytes,
        admission_rule: AdmissionRule::Always,
        bypass_rule: BypassRule::None,
        prewarm_set: vec![],
        ttl_class_rules: vec![],
        cache_key_rules: vec![],
    };
    let base_obj = eval_fn(&base_ir).unwrap_or(0.0);

    // Build ScoredObjects for QUBO: each DSL variable is an "object"
    let linear_terms: Vec<ScoredObject> = vars
        .iter()
        .map(|v| {
            let mut ir = base_ir.clone();
            (v.apply)(&mut ir, &ctx);
            let obj = eval_fn(&ir).unwrap_or(0.0);
            let benefit = obj - base_obj;
            qc_model::object::ScoredObject {
                object_id: v.label.clone(),
                cache_key: format!("{}:{}", v.dimension, v.label),
                size_bytes: 1, // uniform size — capacity encodes one-hot constraint
                net_benefit: benefit,
                score_breakdown: qc_model::object::ScoreBreakdown {
                    expected_hit_benefit: benefit,
                    freshness_cost: 0.0,
                    net_benefit: benefit,
                    capacity_shadow_cost: None,
                },
            }
        })
        .collect();

    // Estimate cross-dimension interactions
    let interactions = estimate_interactions(&vars, &ctx, &eval_fn);

    // One-hot constraints: within each dimension, at most 1 variable active.
    // Encode as strong negative pairwise weights for same-dimension pairs.
    let mut all_interactions = interactions;
    for i in 0..vars.len() {
        for j in (i + 1)..vars.len() {
            if vars[i].dimension == vars[j].dimension {
                all_interactions.push(PairwiseInteraction {
                    i: i as u32,
                    j: j as u32,
                    weight: -1000.0, // strong penalty for selecting two from same dimension
                });
            }
        }
    }

    let problem = QuadraticProblem {
        objects: linear_terms,
        interactions: all_interactions,
        capacity_bytes: vars.len() as u64, // no real capacity constraint
    };

    let solver = SimulatedAnnealingSolver {
        max_iterations: 5000,
        ..SimulatedAnnealingSolver::default()
    };
    let result: SolverResult = solver.solve(&problem)?;

    // Decode: build PolicyIR from active variables
    let mut best_ir = base_ir;
    for (idx, decision) in result.decisions.iter().enumerate() {
        if decision.cache && idx < vars.len() {
            (vars[idx].apply)(&mut best_ir, &ctx);
        }
    }
    let best_objective = eval_fn(&best_ir).unwrap_or(result.objective_value);

    let search_time_ms = start.elapsed().as_millis() as u64;

    Ok(PolicySearchResult {
        best_ir,
        best_objective,
        candidates_evaluated: result.decisions.len(),
        search_time_ms,
        top_candidates: vec![], // QUBO returns single best
    })
}
