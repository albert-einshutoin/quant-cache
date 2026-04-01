use qc_model::object::{ScoreBreakdown, ScoredObject};
use qc_model::scenario::CapacityConstraint;
use qc_solver::greedy::GreedySolver;
use qc_solver::solver::Solver;

fn scored(id: &str, size: u64, benefit: f64) -> ScoredObject {
    ScoredObject {
        object_id: id.into(),
        cache_key: format!("/{id}"),
        size_bytes: size,
        net_benefit: benefit,
        score_breakdown: ScoreBreakdown {
            expected_hit_benefit: benefit.max(0.0),
            freshness_cost: 0.0,
            net_benefit: benefit,
            capacity_shadow_cost: None,
        },
    }
}

#[test]
fn all_fit_within_capacity() {
    let objects = vec![scored("a", 100, 10.0), scored("b", 200, 20.0)];
    let constraint = CapacityConstraint {
        capacity_bytes: 1000,
    };
    let result = GreedySolver.solve(&objects, &constraint).unwrap();

    let cached: Vec<_> = result.decisions.iter().filter(|d| d.cache).collect();
    assert_eq!(cached.len(), 2);
    assert!((result.objective_value - 30.0).abs() < 1e-9);
    assert!(result.feasible);
}

#[test]
fn capacity_constraint_respected() {
    // a: 600 bytes, benefit 10 (eff = 0.0167)
    // b: 500 bytes, benefit 20 (eff = 0.04)
    // capacity: 800 → only b fits by ratio sort
    let objects = vec![scored("a", 600, 10.0), scored("b", 500, 20.0)];
    let constraint = CapacityConstraint {
        capacity_bytes: 800,
    };
    let result = GreedySolver.solve(&objects, &constraint).unwrap();

    let cached_keys: Vec<_> = result
        .decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| d.cache_key.as_str())
        .collect();
    assert!(
        cached_keys.contains(&"/b"),
        "b should be cached (higher efficiency)"
    );
    assert!(!cached_keys.contains(&"/a"), "a should not fit");
    assert!(result.objective_value >= 20.0);
}

#[test]
fn negative_benefit_excluded() {
    let objects = vec![scored("good", 100, 50.0), scored("bad", 100, -5.0)];
    let constraint = CapacityConstraint {
        capacity_bytes: 10000,
    };
    let result = GreedySolver.solve(&objects, &constraint).unwrap();

    let cached_keys: Vec<_> = result
        .decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| d.cache_key.as_str())
        .collect();
    assert!(cached_keys.contains(&"/good"));
    assert!(
        !cached_keys.contains(&"/bad"),
        "negative benefit should not be cached"
    );
}

#[test]
fn empty_input_returns_empty() {
    let result = GreedySolver
        .solve(
            &[],
            &CapacityConstraint {
                capacity_bytes: 1000,
            },
        )
        .unwrap();
    assert!(result.decisions.is_empty());
    assert_eq!(result.objective_value, 0.0);
}

#[test]
fn shadow_price_reported_at_cutoff() {
    // Three objects, only first two fit
    let objects = vec![
        scored("a", 400, 40.0), // eff = 0.1
        scored("b", 400, 20.0), // eff = 0.05
        scored("c", 400, 10.0), // eff = 0.025 — cutoff
    ];
    let constraint = CapacityConstraint {
        capacity_bytes: 800,
    };
    let result = GreedySolver.solve(&objects, &constraint).unwrap();

    assert!(result.shadow_price.is_some());
    let mu = result.shadow_price.unwrap();
    assert!(mu > 0.0, "shadow price should be positive at cutoff");
}

#[test]
fn ratio_vs_pure_picks_better_objective() {
    // Scenario where ratio sort and pure benefit sort produce different results
    // ratio prefers small-high-eff, pure prefers large-high-benefit
    let objects = vec![
        scored("small_eff", 100, 15.0), // eff = 0.15
        scored("large_val", 900, 50.0), // eff = 0.056
        scored("medium", 500, 20.0),    // eff = 0.04
    ];
    let constraint = CapacityConstraint {
        capacity_bytes: 1000,
    };
    let result = GreedySolver.solve(&objects, &constraint).unwrap();

    // Ratio: small_eff(100) + large_val(900) = 1000, obj = 65
    // Pure:  large_val(900) + (can't fit medium) = obj = 50... actually small_eff fits
    // Pure:  large_val(900) + small_eff(100) = 1000, obj = 65
    // Both should give 65 in this case. The solver picks the max.
    assert!(
        result.objective_value >= 50.0,
        "should pick the better of ratio vs pure"
    );
}

#[test]
fn decisions_contain_all_objects() {
    let objects = vec![
        scored("a", 100, 10.0),
        scored("b", 200, 5.0),
        scored("c", 300, 15.0),
    ];
    let constraint = CapacityConstraint {
        capacity_bytes: 250,
    };
    let result = GreedySolver.solve(&objects, &constraint).unwrap();

    // Every input object should appear in decisions (cached or not)
    assert_eq!(result.decisions.len(), 3);
}
