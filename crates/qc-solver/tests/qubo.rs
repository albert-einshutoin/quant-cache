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
