use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("invalid set syntax; use '<signal> <value>' or '<signal>=<value>' pairs")]
    InvalidSetSyntax,
    #[error("command failed: {0}")]
    CommandFailed(String),
}
