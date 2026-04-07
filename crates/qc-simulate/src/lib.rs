//! Trace replay simulation engine for quant-cache.
//!
//! Provides:
//! - [`engine::TraceReplayEngine`] — replay traces against any [`engine::CachePolicy`]
//! - [`baselines`] — LRU, GDSF, SIEVE, S3-FIFO, Belady, economic hybrid policies
//! - [`comparator::Comparator`] — multi-policy side-by-side comparison
//! - [`ir_policy::IrPolicy`] — PolicyIR-driven cache policy for deployment evaluation
//! - [`synthetic`] — trace generation and feature aggregation
//! - [`reuse_distance`] / [`co_access`] — V2 scoring inputs

pub mod baselines;
pub mod co_access;
pub mod compact_baselines;
pub mod comparator;
pub mod engine;
pub mod error;
pub mod group_interactions;
pub mod ir_policy;
pub mod reuse_distance;
pub mod synthetic;
