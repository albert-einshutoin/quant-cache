/* quant-cache C API — v0.3.0
 *
 * Economic cache scoring and optimization for CDN operators.
 * Link with -lqc_ffi (shared: libqc_ffi.so / static: libqc_ffi.a)
 *
 * Usage:
 *   QcObjectFeatures features[] = { ... };
 *   QcConfig config = { .capacity_bytes = 50000000, ... };
 *   QcSolveResult result = qc_solve(features, 2, &config);
 *   if (result.error) { fprintf(stderr, "%s\n", result.error); }
 *   for (size_t i = 0; i < result.count; i++) {
 *       if (result.objects[i].cache) {
 *           printf("CACHE %s (benefit: %.2f)\n",
 *                  result.objects[i].cache_key, result.objects[i].net_benefit);
 *       }
 *   }
 *   qc_free_result(&result);
 */

#ifndef QUANT_CACHE_H
#define QUANT_CACHE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Input: object features for scoring */
typedef struct {
    const char *cache_key;
    uint64_t size_bytes;
    uint64_t request_count;
    double avg_origin_cost;
    double avg_latency_saving_ms;
    uint64_t ttl_seconds;
    double update_rate;
    bool eligible;
} QcObjectFeatures;

/* Input: scenario configuration */
typedef struct {
    uint64_t capacity_bytes;
    uint64_t time_window_seconds;
    double latency_value_per_ms;
    double default_stale_penalty;
} QcConfig;

/* Output: scored object with cache decision */
typedef struct {
    char *cache_key;       /* heap-allocated, freed by qc_free_result */
    uint64_t size_bytes;
    double net_benefit;
    bool cache;
} QcScoredObject;

/* Output: solve result */
typedef struct {
    QcScoredObject *objects;  /* array of count elements */
    size_t count;
    double objective_value;
    uint64_t solve_time_ms;
    char *error;              /* null if success, heap-allocated */
} QcSolveResult;

/*
 * Score and solve: given object features and config, return cache decisions.
 *
 * @param features  Array of object features
 * @param count     Number of elements in features
 * @param config    Scenario configuration
 * @return          Solve result (caller must free via qc_free_result)
 */
QcSolveResult qc_solve(const QcObjectFeatures *features, size_t count,
                        const QcConfig *config);

/*
 * Free a solve result returned by qc_solve.
 */
void qc_free_result(QcSolveResult *result);

/*
 * Return the library version string (static, do not free).
 */
const char *qc_version(void);

#ifdef __cplusplus
}
#endif

#endif /* QUANT_CACHE_H */
