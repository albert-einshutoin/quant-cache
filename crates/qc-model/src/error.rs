use thiserror::Error;

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("invalid capacity: {0}")]
    InvalidCapacity(String),

    #[error("invalid time window: {0}")]
    InvalidTimeWindow(String),

    #[error("invalid parameter: {field} = {value}")]
    InvalidParameter { field: String, value: String },
}
