use proptest::prelude::*;
use qc_model::object::{ScoreBreakdown, ScoredObject};
use qc_model::scenario::CapacityConstraint;
use qc_solver::greedy::GreedySolver;
use qc_solver::ilp::ExactIlpSolver;
use qc_solver::solver::Solver;

fn arb_scored_object() -> impl Strategy<Value = ScoredObject> {
    (
        "[a-z]{3,6}",    // object_id
        1u64..1_000_000, // size_bytes
        -10.0f64..100.0, // net_benefit
        0.0f64..200.0,   // expected_hit_benefit
    )
        .prop_map(|(id, size, benefit, hit_benefit)| ScoredObject {
            object_id: id.clone(),
            cache_key: format!("/{id}"),
            size_bytes: size,
            net_benefit: benefit,
            score_breakdown: ScoreBreakdown {
                expected_hit_benefit: hit_benefit,
                freshness_cost: (hit_benefit - benefit).max(0.0),
                net_benefit: benefit,
                capacity_shadow_cost: None,
            },
        })
}

fn arb_problem() -> impl Strategy<Value = (Vec<ScoredObject>, CapacityConstraint)> {
    (
        prop::collection::vec(arb_scored_object(), 1..50),
        1u64..10_000_000,
    )
        .prop_map(|(mut objects, cap)| {
            // Ensure unique cache_keys by appending index
            for (i, obj) in objects.iter_mut().enumerate() {
                obj.cache_key = format!("/{}-{i}", obj.object_id);
            }
            (
                objects,
                CapacityConstraint {
                    capacity_bytes: cap,
                },
            )
        })
}

proptest! {
    /// S1: Greedy never exceeds capacity constraint.
    #[test]
    fn greedy_respects_capacity((objects, constraint) in arb_problem()) {
        let result = GreedySolver.solve(&objects, &constraint).unwrap();
        let size_map: std::collections::HashMap<_, _> = objects.iter()
            .map(|o| (o.cache_key.as_str(), o.size_bytes))
            .collect();
        let used: u64 = result.decisions.iter()
            .filter(|d| d.cache)
            .map(|d| size_map.get(d.cache_key.as_str()).copied().unwrap_or(0))
            .sum();
        prop_assert!(used <= constraint.capacity_bytes,
            "used {} > capacity {}", used, constraint.capacity_bytes);
    }

    /// S3: Greedy does not cache objects with net_benefit <= 0.
    #[test]
    fn greedy_excludes_negative_benefit((objects, constraint) in arb_problem()) {
        let result = GreedySolver.solve(&objects, &constraint).unwrap();
        let benefit_map: std::collections::HashMap<_, _> = objects.iter()
            .map(|o| (o.cache_key.as_str(), o.net_benefit))
            .collect();
        for decision in &result.decisions {
            if let Some(&benefit) = benefit_map.get(decision.cache_key.as_str()) {
                if benefit <= 0.0 {
                    prop_assert!(!decision.cache,
                        "object {} with benefit {} should not be cached",
                        decision.cache_key, benefit);
                }
            }
        }
    }

    /// S4: Greedy is deterministic.
    #[test]
    fn greedy_is_deterministic((objects, constraint) in arb_problem()) {
        let r1 = GreedySolver.solve(&objects, &constraint).unwrap();
        let r2 = GreedySolver.solve(&objects, &constraint).unwrap();
        prop_assert_eq!(r1.objective_value, r2.objective_value);
        prop_assert_eq!(r1.decisions.len(), r2.decisions.len());
        for (d1, d2) in r1.decisions.iter().zip(r2.decisions.iter()) {
            prop_assert_eq!(d1.cache, d2.cache);
        }
    }

    /// S5: Increasing capacity never decreases objective (monotonicity).
    #[test]
    fn greedy_monotone_in_capacity(
        (objects, _) in arb_problem(),
        cap1 in 1u64..5_000_000,
        cap2 in 5_000_001u64..10_000_000,
    ) {
        let small = CapacityConstraint { capacity_bytes: cap1 };
        let large = CapacityConstraint { capacity_bytes: cap2 };
        let r_small = GreedySolver.solve(&objects, &small).unwrap();
        let r_large = GreedySolver.solve(&objects, &large).unwrap();
        prop_assert!(r_large.objective_value >= r_small.objective_value - 1e-9,
            "larger capacity {} gave lower objective {} vs {}",
            cap2, r_large.objective_value, r_small.objective_value);
    }

    /// S6: ILP >= Greedy (on small instances).
    /// Uses arb_problem() to guarantee unique cache_keys — both solvers
    /// assume unique keys, and duplicates cause undefined behavior.
    #[test]
    fn ilp_at_least_as_good_as_greedy(
        (objects, constraint) in arb_problem(),
    ) {
        let g = GreedySolver.solve(&objects, &constraint).unwrap();
        let i = ExactIlpSolver.solve(&objects, &constraint).unwrap();
        // HiGHS MIP solver has a default optimality tolerance (~0.01%),
        // so allow a small relative gap rather than exact comparison.
        let tolerance = g.objective_value.abs() * 1e-3 + 1e-9;
        prop_assert!(i.objective_value >= g.objective_value - tolerance,
            "ILP {} < Greedy {} (tolerance {})", i.objective_value, g.objective_value, tolerance);
    }
}
