use thiserror::Error;

#[derive(Debug, Error)]
pub enum SolverError {
    #[error("no feasible solution found")]
    Infeasible,

    #[error("solver failed: {0}")]
    SolverFailure(String),

    #[error("scoring error: {0}")]
    ScoringError(String),
}
