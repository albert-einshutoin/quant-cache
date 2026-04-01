use thiserror::Error;

#[derive(Debug, Error)]
pub enum SimulateError {
    #[error("empty trace: no events to replay")]
    EmptyTrace,

    #[error("invalid trace event: {0}")]
    InvalidEvent(String),

    #[error("generation error: {0}")]
    GenerationError(String),
}
