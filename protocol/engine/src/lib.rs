//! Exact captured-stream admission and native validation of the shared schema.

mod stream;
pub use stream::*;

pub mod json;
pub mod number;
pub mod runtime;
pub mod schema;

#[cfg(test)]
#[path = "../schema_compiler.rs"]
mod schema_compiler;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "state", content = "cause", rename_all = "snake_case")]
pub enum ContractError {
    InvalidRecord(String),
    InvalidDefinition(String),
    InvalidConfiguration(String),
    ValidationUnavailable(String),
}

pub type ContractResult<T> = Result<T, ContractError>;

impl std::fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRecord(cause)
            | Self::InvalidDefinition(cause)
            | Self::InvalidConfiguration(cause)
            | Self::ValidationUnavailable(cause) => formatter.write_str(cause),
        }
    }
}

impl std::error::Error for ContractError {}
