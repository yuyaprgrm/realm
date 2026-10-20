#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("nix error: {0}")]
    Nix(String),
    #[error("bwrap error: {0}")]
    Bwrap(String),
    #[error("spec error: {0}")]
    Spec(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
