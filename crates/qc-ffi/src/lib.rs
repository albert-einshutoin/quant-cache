//! C FFI bindings for quant-cache scoring and solving.
//!
//! Exposes `qc_score_objects` and `qc_solve_greedy` as C-callable functions.
//! Build as `cdylib` (shared) or `staticlib` (static) for linking from
//! C, Go, Python ctypes, Nginx modules, or Envoy filters.
//!
//! # Memory model
//!
//! All returned pointers are heap-allocated and must be freed by the caller
//! via the corresponding `qc_free_*` function.

use std::ffi::CStr;
use std::os::raw::c_char;
use std::slice;

use qc_solver::solver::Solver;

// ── Input types ────────────────────────────────────────────────────

/// C-compatible object features for scoring.
#[repr(C)]
pub struct QcObjectFeatures {
    pub cache_key: *const c_char,
    pub size_bytes: u64,
    pub request_count: u64,
    pub avg_origin_cost: f64,
    pub avg_latency_saving_ms: f64,
    pub ttl_seconds: u64,
    pub update_rate: f64,
    pub eligible: bool,
}

/// C-compatible scenario config.
#[repr(C)]
pub struct QcConfig {
    pub capacity_bytes: u64,
    pub time_window_seconds: u64,
    pub latency_value_per_ms: f64,
    pub default_stale_penalty: f64,
}

// ── Output types ───────────────────────────────────────────────────

/// C-compatible scored result for a single object.
#[repr(C)]
pub struct QcScoredObject {
    pub cache_key: *mut c_char,
    pub size_bytes: u64,
    pub net_benefit: f64,
    pub cache: bool,
}

/// C-compatible solve result.
#[repr(C)]
pub struct QcSolveResult {
    pub objects: *mut QcScoredObject,
    pub count: usize,
    pub objective_value: f64,
    pub solve_time_ms: u64,
    pub error: *mut c_char, // null if success
}

// ── Core API ───────────────────────────────────────────────────────

/// Score and solve: given object features and config, return cache decisions.
///
/// # Safety
/// - `features` must point to a valid array of `count` `QcObjectFeatures`.
/// - All `cache_key` pointers in features must be valid null-terminated C strings.
/// - Caller must free the result via `qc_free_result`.
#[no_mangle]
pub unsafe extern "C" fn qc_solve(
    features: *const QcObjectFeatures,
    count: usize,
    config: *const QcConfig,
) -> QcSolveResult {
    let result = std::panic::catch_unwind(|| solve_impl(features, count, config));
    match result {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => error_result(&e),
        Err(_) => error_result("panic in qc_solve"),
    }
}

fn solve_impl(
    features_ptr: *const QcObjectFeatures,
    count: usize,
    config_ptr: *const QcConfig,
) -> Result<QcSolveResult, String> {
    if features_ptr.is_null() || config_ptr.is_null() {
        return Err("null pointer".into());
    }

    let features_slice = unsafe { slice::from_raw_parts(features_ptr, count) };
    let config = unsafe { &*config_ptr };

    // Convert C features to Rust ObjectFeatures
    let rust_features: Vec<qc_model::object::ObjectFeatures> = features_slice
        .iter()
        .map(|f| {
            let key = unsafe { CStr::from_ptr(f.cache_key) }
                .to_str()
                .unwrap_or("unknown")
                .to_string();
            qc_model::object::ObjectFeatures {
                object_id: key.clone(),
                cache_key: key,
                size_bytes: f.size_bytes,
                eligible_for_cache: f.eligible,
                request_count: f.request_count,
                request_rate: f.request_count as f64 / config.time_window_seconds.max(1) as f64,
                avg_response_bytes: f.size_bytes,
                avg_origin_cost: f.avg_origin_cost,
                avg_latency_saving_ms: f.avg_latency_saving_ms,
                ttl_seconds: f.ttl_seconds,
                update_rate: f.update_rate,
                last_modified: None,
                stale_penalty_class: qc_model::scenario::StalePenaltyClass::Medium,
                purge_group: None,
                origin_group: None,
                mean_reuse_distance: None,
                reuse_distance_p50: None,
                reuse_distance_p95: None,
            }
        })
        .collect();

    // Build ScenarioConfig
    let scenario_config = qc_model::scenario::ScenarioConfig {
        capacity_bytes: config.capacity_bytes,
        time_window_seconds: config.time_window_seconds,
        latency_value_per_ms: config.latency_value_per_ms,
        freshness_model: qc_model::scenario::FreshnessModel::TtlOnly {
            stale_penalty: qc_model::scenario::StalePenaltyConfig {
                default_class: qc_model::scenario::StalePenaltyClass::Medium,
                cost_overrides: qc_model::scenario::StaleCostOverrides::default(),
            },
        },
        scoring_version: qc_model::scenario::ScoringVersion::V1Frequency,
    };

    // Score
    let scored = qc_solver::score::BenefitCalculator::score_all(&rust_features, &scenario_config)
        .map_err(|e| format!("scoring error: {e}"))?;

    // Solve
    let constraint = qc_model::scenario::CapacityConstraint {
        capacity_bytes: config.capacity_bytes,
    };
    let result = qc_solver::greedy::GreedySolver
        .solve(&scored, &constraint)
        .map_err(|e| format!("solver error: {e}"))?;

    // Build C output
    let mut out_objects: Vec<QcScoredObject> = result
        .decisions
        .iter()
        .map(|d| {
            let key_cstr = std::ffi::CString::new(d.cache_key.as_str()).unwrap_or_default();
            QcScoredObject {
                cache_key: key_cstr.into_raw(),
                size_bytes: d.size_bytes,
                net_benefit: d.score,
                cache: d.cache,
            }
        })
        .collect();

    let ptr = out_objects.as_mut_ptr();
    let len = out_objects.len();
    std::mem::forget(out_objects);

    Ok(QcSolveResult {
        objects: ptr,
        count: len,
        objective_value: result.objective_value,
        solve_time_ms: result.solve_time_ms,
        error: std::ptr::null_mut(),
    })
}

fn error_result(msg: &str) -> QcSolveResult {
    let err = std::ffi::CString::new(msg).unwrap_or_default();
    QcSolveResult {
        objects: std::ptr::null_mut(),
        count: 0,
        objective_value: 0.0,
        solve_time_ms: 0,
        error: err.into_raw(),
    }
}

// ── Free functions ─────────────────────────────────────────────────

/// Free a solve result returned by `qc_solve`.
///
/// # Safety
/// Must only be called with a result previously returned by `qc_solve`.
#[no_mangle]
pub unsafe extern "C" fn qc_free_result(result: *mut QcSolveResult) {
    if result.is_null() {
        return;
    }
    let r = &*result;

    // Free each object's cache_key
    if !r.objects.is_null() {
        let objects = Vec::from_raw_parts(r.objects, r.count, r.count);
        for obj in &objects {
            if !obj.cache_key.is_null() {
                let _ = std::ffi::CString::from_raw(obj.cache_key);
            }
        }
    }

    // Free error string
    if !r.error.is_null() {
        let _ = std::ffi::CString::from_raw(r.error);
    }
}

/// Return the library version string.
///
/// # Safety
/// Returns a static string. Do not free.
#[no_mangle]
pub extern "C" fn qc_version() -> *const c_char {
    static VERSION: &[u8] = b"0.3.0\0";
    VERSION.as_ptr() as *const c_char
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solve_basic() {
        let key1 = std::ffi::CString::new("/img/logo.png").unwrap();
        let key2 = std::ffi::CString::new("/api/data").unwrap();

        let features = [
            QcObjectFeatures {
                cache_key: key1.as_ptr(),
                size_bytes: 1000,
                request_count: 100,
                avg_origin_cost: 0.003,
                avg_latency_saving_ms: 50.0,
                ttl_seconds: 3600,
                update_rate: 0.0,
                eligible: true,
            },
            QcObjectFeatures {
                cache_key: key2.as_ptr(),
                size_bytes: 500,
                request_count: 50,
                avg_origin_cost: 0.005,
                avg_latency_saving_ms: 30.0,
                ttl_seconds: 3600,
                update_rate: 0.0,
                eligible: true,
            },
        ];

        let config = QcConfig {
            capacity_bytes: 10000,
            time_window_seconds: 86400,
            latency_value_per_ms: 0.0001,
            default_stale_penalty: 0.0,
        };

        let result = unsafe { qc_solve(features.as_ptr(), features.len(), &config) };

        assert!(result.error.is_null(), "should not error");
        assert_eq!(result.count, 2);
        assert!(result.objective_value > 0.0);

        // Clean up
        unsafe {
            qc_free_result(&result as *const _ as *mut _);
        }
    }

    #[test]
    fn solve_null_ptr_returns_error() {
        let config = QcConfig {
            capacity_bytes: 10000,
            time_window_seconds: 86400,
            latency_value_per_ms: 0.0001,
            default_stale_penalty: 0.0,
        };
        let result = unsafe { qc_solve(std::ptr::null(), 0, &config) };
        assert!(!result.error.is_null());
        unsafe {
            qc_free_result(&result as *const _ as *mut _);
        }
    }

    #[test]
    fn version_returns_string() {
        let v = qc_version();
        let s = unsafe { CStr::from_ptr(v) }.to_str().unwrap();
        assert_eq!(s, "0.3.0");
    }
}
