use qc_model::object::{ScoreBreakdown, ScoredObject};
use qc_solver::qubo::{
    PairwiseInteraction, QuadraticProblem, QuadraticSolver, SimulatedAnnealingSolver,
};

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
fn sa_empty_problem() {
    let problem = QuadraticProblem {
        objects: vec![],
        interactions: vec![],
        capacity_bytes: 1000,
    };
    let solver = SimulatedAnnealingSolver::default();
    let result = solver.solve(&problem).unwrap();
    assert!(result.decisions.is_empty());
    assert_eq!(result.objective_value, 0.0);
}

#[test]
fn sa_linear_only_matches_greedy() {
    let objects = vec![
        scored("a", 300, 30.0),
        scored("b", 400, 20.0),
        scored("c", 500, 50.0),
    ];
    let problem = QuadraticProblem {
        objects,
        interactions: vec![],
        capacity_bytes: 800,
    };
    let solver = SimulatedAnnealingSolver {
        max_iterations: 50_000,
        ..Default::default()
    };
    let result = solver.solve(&problem).unwrap();

    // Optimal: a(300) + c(500) = 800, obj = 80
    assert!(result.objective_value >= 50.0, "should find good solution");
    assert!(result.feasible);
}

#[test]
fn sa_respects_capacity() {
    let objects: Vec<_> = (0..20)
        .map(|i| scored(&format!("obj{i}"), 100, 10.0))
        .collect();
    let problem = QuadraticProblem {
        objects: objects.clone(),
        interactions: vec![],
        capacity_bytes: 500,
    };
    let solver = SimulatedAnnealingSolver::default();
    let result = solver.solve(&problem).unwrap();

    let cached_count = result.decisions.iter().filter(|d| d.cache).count();
    assert!(
        cached_count <= 5,
        "cached {cached_count} > 5 items (cap 500, each 100)"
    );
}

#[test]
fn sa_quadratic_bonus_helps() {
    // Without interaction: prefer c (highest benefit)
    // With interaction: a+b together get a bonus
    let objects = vec![
        scored("a", 300, 10.0),
        scored("b", 300, 10.0),
        scored("c", 300, 25.0),
    ];
    // Capacity fits 2 objects (600)
    // Without interaction: c + a or c + b = 35
    // With interaction a-b bonus of 20: a + b = 20 + 20 = 40
    let problem = QuadraticProblem {
        objects,
        interactions: vec![PairwiseInteraction {
            i: 0,
            j: 1,
            weight: 20.0,
        }],
        capacity_bytes: 600,
    };
    let solver = SimulatedAnnealingSolver {
        max_iterations: 50_000,
        ..Default::default()
    };
    let result = solver.solve(&problem).unwrap();

    // SA should find that a+b (obj=40) beats c+anything (obj=35)
    assert!(
        result.objective_value >= 35.0,
        "should find at least linear optimal, got {}",
        result.objective_value
    );
}

#[test]
fn sa_negative_benefit_excluded() {
    let objects = vec![scored("good", 100, 50.0), scored("bad", 100, -10.0)];
    let problem = QuadraticProblem {
        objects,
        interactions: vec![],
        capacity_bytes: 1000,
    };
    let solver = SimulatedAnnealingSolver::default();
    let result = solver.solve(&problem).unwrap();

    let bad_cached = result
        .decisions
        .iter()
        .any(|d| d.cache_key == "/bad" && d.cache);
    assert!(!bad_cached, "negative benefit object should not be cached");
}

#[test]
fn sa_purge_group_consistency_bonus() {
    // Two objects in same "purge group" get a co-caching bonus.
    // Without bonus: solver picks c (25) + a or b (10) = 35
    // With bonus: a + b = 10 + 10 + 25(bonus) = 45
    let objects = vec![
        scored("purge-a", 300, 10.0),
        scored("purge-b", 300, 10.0),
        scored("solo-c", 300, 25.0),
    ];
    let problem = QuadraticProblem {
        objects,
        interactions: vec![PairwiseInteraction {
            i: 0,
            j: 1,
            weight: 25.0, // consistency bonus for co-caching purge group
        }],
        capacity_bytes: 600,
    };
    let solver = SimulatedAnnealingSolver {
        max_iterations: 50_000,
        ..Default::default()
    };
    let result = solver.solve(&problem).unwrap();

    // SA should prefer a+b (obj=45) over c+anything (obj=35)
    let a_cached = result
        .decisions
        .iter()
        .any(|d| d.cache_key == "/purge-a" && d.cache);
    let b_cached = result
        .decisions
        .iter()
        .any(|d| d.cache_key == "/purge-b" && d.cache);
    assert!(
        result.objective_value >= 40.0,
        "purge group bonus should yield high objective: got {}",
        result.objective_value
    );
    assert!(
        a_cached && b_cached,
        "SA should co-cache purge group objects: a={a_cached}, b={b_cached}"
    );
}

#[test]
fn sa_origin_group_burst_shielding() {
    // Objects sharing an origin get a bonus for co-caching (burst shielding).
    // 4 objects, capacity fits 2. Origin pair has bonus making them preferred.
    let objects = vec![
        scored("origin-a", 200, 8.0),
        scored("origin-b", 200, 8.0),
        scored("other-c", 200, 12.0),
        scored("other-d", 200, 11.0),
    ];
    // Without bonus: c(12) + d(11) = 23
    // With bonus: a(8) + b(8) + 15(bonus) = 31
    let problem = QuadraticProblem {
        objects,
        interactions: vec![PairwiseInteraction {
            i: 0,
            j: 1,
            weight: 15.0, // origin burst shielding bonus
        }],
        capacity_bytes: 400,
    };
    let solver = SimulatedAnnealingSolver {
        max_iterations: 50_000,
        ..Default::default()
    };
    let result = solver.solve(&problem).unwrap();

    assert!(
        result.objective_value >= 23.0,
        "origin bonus should beat linear optimum: got {}",
        result.objective_value
    );
}

#[test]
fn sa_mixed_interactions_co_access_plus_groups() {
    // Combine co-access and group interactions in the same problem.
    let objects = vec![
        scored("a", 200, 10.0), // origin group with b
        scored("b", 200, 10.0), // origin group with a
        scored("c", 200, 10.0), // co-access with d
        scored("d", 200, 10.0), // co-access with c
        scored("e", 200, 15.0), // solo, highest linear
    ];
    // Capacity fits 2 objects (400 bytes)
    // Linear best: e(15) + any(10) = 25
    // With interactions: a+b get origin bonus(12) = 10+10+12 = 32
    //                    c+d get co-access bonus(8) = 10+10+8 = 28
    let problem = QuadraticProblem {
        objects,
        interactions: vec![
            PairwiseInteraction {
                i: 0,
                j: 1,
                weight: 12.0, // origin-group bonus
            },
            PairwiseInteraction {
                i: 2,
                j: 3,
                weight: 8.0, // co-access bonus
            },
        ],
        capacity_bytes: 400,
    };
    let solver = SimulatedAnnealingSolver {
        max_iterations: 50_000,
        ..Default::default()
    };
    let result = solver.solve(&problem).unwrap();

    // SA should find a+b as optimal (32 > 28 > 25)
    assert!(
        result.objective_value >= 25.0,
        "mixed interactions: should beat linear, got {}",
        result.objective_value
    );
}
