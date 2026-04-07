//! Core types and configuration for the quant-cache economic evaluation framework.
//!
//! This crate defines the data model used across the entire quant-cache pipeline:
//! trace events, object features, scoring results, scenario configs, policy decisions,
//! and the PolicyIR intermediate representation for deployment compilation.

pub mod error;
pub mod metrics;
pub mod object;
pub mod origin_cost;
pub mod policy;
pub mod policy_ir;
pub mod preset;
pub mod scenario;
pub mod trace;
