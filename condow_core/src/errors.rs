use thiserror::Error;

#[derive(Error, Debug)]
pub enum DownloadRangeError {
    #[error("invalid range: {0}")]
    InvalidRange(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("access denied: {0}")]
    AccessDenied(String),
    #[error("error: {0}")]
    Remote(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("error: {0}")]
    Other(String),
}

#[derive(Error, Debug)]
pub enum DownloadFileError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("access denied: {0}")]
    AccessDenied(String),
    #[error("error: {0}")]
    Remote(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("error: {0}")]
    Other(String),
}

impl From<DownloadFileError> for DownloadRangeError {
    fn from(dfe: DownloadFileError) -> Self {
        match dfe {
            DownloadFileError::NotFound(msg) => DownloadRangeError::NotFound(msg),
            DownloadFileError::AccessDenied(msg) => DownloadRangeError::AccessDenied(msg),
            DownloadFileError::Remote(msg) => DownloadRangeError::Remote(msg),
            DownloadFileError::Io(msg) => DownloadRangeError::Io(msg),
            DownloadFileError::Other(msg) => DownloadRangeError::Other(msg),
        }
    }
}

impl From<DownloadRangeError> for DownloadFileError {
    fn from(dfe: DownloadRangeError) -> Self {
        match dfe {
            DownloadRangeError::InvalidRange(msg) => DownloadFileError::Other(msg),
            DownloadRangeError::NotFound(msg) => DownloadFileError::NotFound(msg),
            DownloadRangeError::AccessDenied(msg) => DownloadFileError::AccessDenied(msg),
            DownloadRangeError::Remote(msg) => DownloadFileError::Remote(msg),
            DownloadRangeError::Io(msg) => DownloadFileError::Io(msg),
            DownloadRangeError::Other(msg) => DownloadFileError::Other(msg),
        }
    }
}

impl From<GetSizeError> for DownloadRangeError {
    fn from(dfe: GetSizeError) -> Self {
        match dfe {
            GetSizeError::NotFound(msg) => DownloadRangeError::NotFound(msg),
            GetSizeError::AccessDenied(msg) => DownloadRangeError::AccessDenied(msg),
            GetSizeError::Remote(msg) => DownloadRangeError::Remote(msg),
            GetSizeError::Io(msg) => DownloadRangeError::Io(msg),
            GetSizeError::Other(msg) => DownloadRangeError::Other(msg),
        }
    }
}

#[derive(Error, Debug)]
pub enum StreamError {
    #[error("invalid range: {0}")]
    InvalidRange(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("access denied: {0}")]
    AccessDenied(String),
    #[error("error: {0}")]
    Remote(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("error: {0}")]
    Other(String),
}

impl From<DownloadRangeError> for StreamError {
    fn from(dre: DownloadRangeError) -> Self {
        match dre {
            DownloadRangeError::InvalidRange(msg) => StreamError::InvalidRange(msg),
            DownloadRangeError::NotFound(msg) => StreamError::NotFound(msg),
            DownloadRangeError::AccessDenied(msg) => StreamError::AccessDenied(msg),
            DownloadRangeError::Remote(msg) => StreamError::Remote(msg),
            DownloadRangeError::Io(msg) => StreamError::Io(msg),
            DownloadRangeError::Other(msg) => StreamError::Other(msg),
        }
    }
}

#[derive(Error, Debug)]
pub enum GetSizeError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("access denied: {0}")]
    AccessDenied(String),
    #[error("error: {0}")]
    Remote(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("error: {0}")]
    Other(String),
}
#[derive(Error, Debug)]
#[error("io error: {0}")]
pub struct IoError(pub String);