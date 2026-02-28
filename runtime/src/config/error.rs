use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config parse failed: {0}")]
    Parse(String),
    #[error("missing recipe: {0}")]
    MissingRecipe(String),
    #[error("invalid recipe step: {0}")]
    InvalidRecipeStep(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
