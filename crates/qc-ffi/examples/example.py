"""
Python example using quant-cache C API via ctypes.

Build the shared library first:
    cargo build --release -p qc-ffi

Then run:
    python3 crates/qc-ffi/examples/example.py
"""

import ctypes
import os
import sys

# Load the shared library
if sys.platform == "darwin":
    lib_name = "libqc_ffi.dylib"
elif sys.platform == "linux":
    lib_name = "libqc_ffi.so"
else:
    lib_name = "qc_ffi.dll"

lib_path = os.path.join(os.path.dirname(__file__), "..", "..", "..", "target", "release", lib_name)
lib = ctypes.CDLL(lib_path)


# Define C types
class QcObjectFeatures(ctypes.Structure):
    _fields_ = [
        ("cache_key", ctypes.c_char_p),
        ("size_bytes", ctypes.c_uint64),
        ("request_count", ctypes.c_uint64),
        ("avg_origin_cost", ctypes.c_double),
        ("avg_latency_saving_ms", ctypes.c_double),
        ("ttl_seconds", ctypes.c_uint64),
        ("update_rate", ctypes.c_double),
        ("eligible", ctypes.c_bool),
    ]


class QcConfig(ctypes.Structure):
    _fields_ = [
        ("capacity_bytes", ctypes.c_uint64),
        ("time_window_seconds", ctypes.c_uint64),
        ("latency_value_per_ms", ctypes.c_double),
        ("default_stale_penalty", ctypes.c_double),
    ]


class QcScoredObject(ctypes.Structure):
    _fields_ = [
        ("cache_key", ctypes.c_char_p),
        ("size_bytes", ctypes.c_uint64),
        ("net_benefit", ctypes.c_double),
        ("cache", ctypes.c_bool),
    ]


class QcSolveResult(ctypes.Structure):
    _fields_ = [
        ("objects", ctypes.POINTER(QcScoredObject)),
        ("count", ctypes.c_size_t),
        ("objective_value", ctypes.c_double),
        ("solve_time_ms", ctypes.c_uint64),
        ("error", ctypes.c_char_p),
    ]


# Set up function signatures
lib.qc_solve.restype = QcSolveResult
lib.qc_solve.argtypes = [
    ctypes.POINTER(QcObjectFeatures),
    ctypes.c_size_t,
    ctypes.POINTER(QcConfig),
]
lib.qc_free_result.restype = None
lib.qc_free_result.argtypes = [ctypes.POINTER(QcSolveResult)]
lib.qc_version.restype = ctypes.c_char_p

# Print version
print(f"quant-cache version: {lib.qc_version().decode()}")

# Create sample objects
features = (QcObjectFeatures * 3)(
    QcObjectFeatures(b"/img/hero.jpg", 524288, 1000, 0.003, 50.0, 3600, 0.0, True),
    QcObjectFeatures(b"/api/products", 4096, 5000, 0.005, 30.0, 3600, 0.01, True),
    QcObjectFeatures(b"/video/intro.mp4", 5242880, 100, 0.01, 100.0, 3600, 0.0, True),
)

config = QcConfig(
    capacity_bytes=1_000_000,  # 1MB cache
    time_window_seconds=86400,
    latency_value_per_ms=0.0001,
    default_stale_penalty=0.0,
)

# Solve
result = lib.qc_solve(features, len(features), ctypes.byref(config))

if result.error:
    print(f"Error: {result.error.decode()}")
else:
    print(f"Objective: ${result.objective_value:.4f}")
    print(f"Solve time: {result.solve_time_ms}ms")
    print(f"Decisions:")
    for i in range(result.count):
        obj = result.objects[i]
        status = "CACHE" if obj.cache else "SKIP"
        print(f"  {status} {obj.cache_key.decode()} (benefit: ${obj.net_benefit:.4f})")

# Free result
lib.qc_free_result(ctypes.byref(result))
