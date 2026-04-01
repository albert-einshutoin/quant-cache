use qc_model::object::{ScoreBreakdown, ScoredObject};
use qc_model::scenario::CapacityConstraint;
use qc_solver::greedy::GreedySolver;
use qc_solver::ilp::ExactIlpSolver;
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

// ── Basic ILP tests ─────────────────────────────────────────────────

#[test]
fn ilp_empty_input() {
    let result = ExactIlpSolver
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
fn ilp_all_fit() {
    let objects = vec![scored("a", 100, 10.0), scored("b", 200, 20.0)];
    let constraint = CapacityConstraint {
        capacity_bytes: 1000,
    };
    let result = ExactIlpSolver.solve(&objects, &constraint).unwrap();

    let cached: Vec<_> = result.decisions.iter().filter(|d| d.cache).collect();
    assert_eq!(cached.len(), 2);
    assert!((result.objective_value - 30.0).abs() < 1e-6);
}

#[test]
fn ilp_capacity_constraint_respected() {
    // Classic 0-1 knapsack: capacity = 50
    // a: size=10, benefit=60  (eff=6.0)
    // b: size=20, benefit=100 (eff=5.0)
    // c: size=30, benefit=120 (eff=4.0)
    // Optimal: a + b = size 30, benefit 160
    //   or a + c = size 40, benefit 180 ← optimal
    let objects = vec![
        scored("a", 10, 60.0),
        scored("b", 20, 100.0),
        scored("c", 30, 120.0),
    ];
    let constraint = CapacityConstraint { capacity_bytes: 50 };
    let result = ExactIlpSolver.solve(&objects, &constraint).unwrap();

    // ILP should find a+c (180) or a+b+c if they fit (they don't: 60 > 50)
    // a+c = 40 <= 50, benefit = 180
    // b+c = 50 <= 50, benefit = 220 ← this is actually better!
    assert!(
        (result.objective_value - 220.0).abs() < 1e-6,
        "optimal is b+c=220, got {}",
        result.objective_value
    );

    let cached_keys: Vec<_> = result
        .decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| d.cache_key.as_str())
        .collect();
    assert!(cached_keys.contains(&"/b"));
    assert!(cached_keys.contains(&"/c"));
}

#[test]
fn ilp_negative_benefit_excluded() {
    let objects = vec![scored("good", 100, 50.0), scored("bad", 100, -5.0)];
    let constraint = CapacityConstraint {
        capacity_bytes: 10000,
    };
    let result = ExactIlpSolver.solve(&objects, &constraint).unwrap();

    let cached_keys: Vec<_> = result
        .decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| d.cache_key.as_str())
        .collect();
    assert!(cached_keys.contains(&"/good"));
    assert!(!cached_keys.contains(&"/bad"));
}

#[test]
fn ilp_decisions_contain_all_objects() {
    let objects = vec![
        scored("a", 100, 10.0),
        scored("b", 200, 5.0),
        scored("c", 300, 15.0),
    ];
    let constraint = CapacityConstraint {
        capacity_bytes: 250,
    };
    let result = ExactIlpSolver.solve(&objects, &constraint).unwrap();
    assert_eq!(result.decisions.len(), 3);
}

// ── ILP >= Greedy invariant ─────────────────────────────────────────

#[test]
fn ilp_at_least_as_good_as_greedy_simple() {
    // Case where greedy ratio sort is suboptimal
    // a: size=6, benefit=6 (eff=1.0) — greedy picks first
    // b: size=5, benefit=5 (eff=1.0) — greedy picks second, total=11, used=11 > 10
    // Actually, let's construct a proper case:
    // capacity=10
    // a: size=6, benefit=7  (eff=1.167)
    // b: size=5, benefit=5  (eff=1.0)
    // c: size=5, benefit=5  (eff=1.0)
    // Greedy by ratio: a(6) + b(5) = 11 > 10, so a(6) + can't fit b → just a, obj=7
    // Actually greedy: a(6), then b doesn't fit, c doesn't fit → obj=7
    // ILP optimal: b+c = 10, obj=10
    let objects = vec![
        scored("a", 6, 7.0),
        scored("b", 5, 5.0),
        scored("c", 5, 5.0),
    ];
    let constraint = CapacityConstraint { capacity_bytes: 10 };

    let greedy_result = GreedySolver.solve(&objects, &constraint).unwrap();
    let ilp_result = ExactIlpSolver.solve(&objects, &constraint).unwrap();

    assert!(
        ilp_result.objective_value >= greedy_result.objective_value - 1e-9,
        "ILP ({}) should be >= greedy ({})",
        ilp_result.objective_value,
        greedy_result.objective_value
    );
}

#[test]
fn ilp_beats_greedy_on_classic_knapsack() {
    // Classic case where greedy by efficiency is suboptimal
    // capacity = 50
    // a: size=10, benefit=60  (eff=6.0) — greedy picks
    // b: size=20, benefit=100 (eff=5.0) — greedy picks, total=30
    // c: size=30, benefit=120 (eff=4.0) — doesn't fit (30+30=60>50)
    // Greedy: a+b = 160
    // ILP: b+c = 220 (size=50, fits exactly)
    let objects = vec![
        scored("a", 10, 60.0),
        scored("b", 20, 100.0),
        scored("c", 30, 120.0),
    ];
    let constraint = CapacityConstraint { capacity_bytes: 50 };

    let greedy_result = GreedySolver.solve(&objects, &constraint).unwrap();
    let ilp_result = ExactIlpSolver.solve(&objects, &constraint).unwrap();

    assert!(
        ilp_result.objective_value >= greedy_result.objective_value,
        "ILP ({}) should be >= greedy ({})",
        ilp_result.objective_value,
        greedy_result.objective_value
    );

    // Calculate optimality gap
    if ilp_result.objective_value > 0.0 {
        let gap = (ilp_result.objective_value - greedy_result.objective_value)
            / ilp_result.objective_value;
        // Just verify the gap is calculable and non-negative
        assert!(gap >= 0.0, "gap should be non-negative");
    }
}

#[test]
fn ilp_greedy_agree_when_greedy_optimal() {
    // When all items fit, both should give the same answer
    let objects = vec![
        scored("a", 100, 10.0),
        scored("b", 200, 20.0),
        scored("c", 300, 30.0),
    ];
    let constraint = CapacityConstraint {
        capacity_bytes: 10000,
    };

    let greedy_result = GreedySolver.solve(&objects, &constraint).unwrap();
    let ilp_result = ExactIlpSolver.solve(&objects, &constraint).unwrap();

    assert!(
        (ilp_result.objective_value - greedy_result.objective_value).abs() < 1e-6,
        "when all fit, ILP ({}) == greedy ({})",
        ilp_result.objective_value,
        greedy_result.objective_value
    );
}

// ── Optimality gap calculation helper ───────────────────────────────

#[test]
fn optimality_gap_calculation() {
    let objects = vec![
        scored("a", 10, 60.0),
        scored("b", 20, 100.0),
        scored("c", 30, 120.0),
    ];
    let constraint = CapacityConstraint { capacity_bytes: 50 };

    let greedy = GreedySolver.solve(&objects, &constraint).unwrap();
    let ilp = ExactIlpSolver.solve(&objects, &constraint).unwrap();

    let gap = (ilp.objective_value - greedy.objective_value) / ilp.objective_value;
    println!(
        "Greedy: {}, ILP: {}, Gap: {:.2}%",
        greedy.objective_value,
        ilp.objective_value,
        gap * 100.0
    );
    assert!(gap >= 0.0);
    assert!(gap <= 1.0);
}
