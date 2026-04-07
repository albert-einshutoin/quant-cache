# Output Format Reference

**Version:** 1.0
**Date:** 2026-04-08

This document defines the stable output formats for quant-cache CLI commands.
These formats are versioned and backward-compatible within the same major version.

---

## trace.csv (RequestTraceEvent)

Output of `qc generate` and `qc import`. Input to all analysis commands.

| Column | Type | Required | Description |
|--------|------|:--------:|-------------|
| schema_version | string | yes | Always "1.0" |
| timestamp | ISO 8601 | yes | Event timestamp (UTC) |
| object_id | string | yes | Logical object identifier |
| cache_key | string | yes | Cache key (URL path + query) |
| object_size_bytes | u64 | yes | Object size in bytes |
| response_bytes | u64 | no | Actual response size (may differ for partial) |
| cache_status | enum | no | Hit, Miss, Expired, Bypass |
| status_code | u16 | no | HTTP status code |
| origin_fetch_cost | f64 | no | Estimated origin cost ($) |
| response_latency_ms | f64 | no | Response latency (ms) |
| region | string | no | Edge region identifier |
| content_type | string | no | Content-Type header |
| version_or_etag | string | no | Version or ETag for stale detection |
| eligible_for_cache | bool | yes | Whether object can be cached |

---

## policy.json (PolicyFile)

Output of `qc optimize`. Input to `qc simulate` and `qc compile --scores`.

```json
{
  "solver": {
    "solver_name": "greedy",
    "objective_value": 42.1234,
    "solve_time_ms": 15,
    "shadow_price": 0.000001,
    "optimality_gap": null,
    "capacity_bytes": 50000000,
    "cached_bytes": 12345678
  },
  "decisions": [
    {
      "cache_key": "/img/logo.png",
      "cache": true,
      "score": 12.34,
      "size_bytes": 1024,
      "score_breakdown": {
        "expected_hit_benefit": 15.0,
        "freshness_cost": 2.66,
        "net_benefit": 12.34,
        "capacity_shadow_cost": null
      }
    }
  ]
}
```

---

## policy_ir.json (PolicyIR)

Output of `qc policy-search`. Input to `qc compile` and `qc deploy-check`.

```json
{
  "backend": "Sieve",
  "capacity_bytes": 50000000,
  "admission_rule": { "ScoreThreshold": { "threshold": 0.5 } },
  "bypass_rule": { "SizeLimit": { "max_bytes": 10000000 } },
  "prewarm_set": ["/popular/page"],
  "ttl_class_rules": [
    { "content_type_pattern": "image/", "ttl_seconds": 86400 }
  ],
  "cache_key_rules": [
    { "pattern": "[?&]utm_[^&]*", "replacement": "" }
  ]
}
```

---

## Compiled config (target-specific)

Output of `qc compile`. Structure varies by target:

### Cloudflare
```json
{
  "_generated_by": "quant-cache v0.3",
  "_target": "cloudflare",
  "ruleset_payload": { ... },
  "cache_key_config": { ... },
  "worker_script": "...",
  "prewarm_urls": ["/path"]
}
```

### CloudFront
```json
{
  "_generated_by": "quant-cache v0.3",
  "_target": "cloudfront",
  "cache_behaviors": [ ... ],
  "cloudfront_function": "...",
  "prewarm_paths": ["/path"]
}
```

### Fastly
```json
{
  "_generated_by": "quant-cache v0.3",
  "_target": "fastly",
  "vcl_snippets": [ ... ],
  "prewarm_urls": ["/path"]
}
```

### Akamai
```json
{
  "_generated_by": "quant-cache v0.3",
  "_target": "akamai",
  "rule_tree": { ... },
  "edgeworker_bundle": "...",
  "prewarm_urls": ["/path"]
}
```

---

## MetricsSummary (JSON)

Output of `qc simulate --output metrics.json`.

```json
{
  "total_requests": 100000,
  "cache_hits": 60000,
  "cache_misses": 40000,
  "hit_ratio": 0.60,
  "byte_hit_ratio": 0.75,
  "bytes_from_cache": 75000000,
  "total_bytes_served": 100000000,
  "origin_egress_bytes": 25000000,
  "estimated_cost_savings": 180.0,
  "policy_objective_value": 150.0,
  "stale_serve_count": 500,
  "stale_serve_rate": 0.005,
  "capacity_utilization": 0.85,
  "solve_time_ms": 15,
  "optimality_gap": null
}
```

---

## Versioning Policy

- Format version is embedded in `schema_version` (trace.csv) and `_generated_by` (compile output)
- Breaking changes to output structure require major version bump
- New optional fields may be added in minor versions
- Removed fields are deprecated for one minor version before removal
