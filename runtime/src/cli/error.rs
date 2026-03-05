use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("missing command; run --help for usage")]
    MissingCommand,
    #[error("invalid set syntax; use '<signal> <value>' or '<signal>=<value>' pairs")]
    InvalidSetSyntax,
    #[error("command failed: {0}")]
    CommandFailed(String),
    #[error("assertion failed: {0}")]
    AssertionFailed(String),
}
