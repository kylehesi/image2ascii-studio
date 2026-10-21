/// Everything that can go wrong in the core. No variant is ever produced by a
/// panic path: malformed input always surfaces as one of these.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not decode image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("image has no pixels")]
    EmptyImage,
    #[error("font error: {0}")]
    Font(String),
    #[error("invalid charset: {0}")]
    Charset(String),
    #[error("invalid option: {0}")]
    InvalidOption(String),
}

pub type Result<T> = std::result::Result<T, Error>;
