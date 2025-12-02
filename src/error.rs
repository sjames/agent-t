use thiserror::Error;

/// Common error type for all tools
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Command failed: {0}")]
    CommandFailed(String),

    #[error("Command timeout")]
    CommandTimeout,

    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("Pattern error: {0}")]
    PatternError(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("{0}")]
    Other(String),
}

impl ToolError {
    pub fn file_not_found(path: impl Into<String>) -> Self {
        Self::FileNotFound(path.into())
    }

    pub fn permission_denied(path: impl Into<String>) -> Self {
        Self::PermissionDenied(path.into())
    }

    pub fn invalid_path(path: impl Into<String>) -> Self {
        Self::InvalidPath(path.into())
    }

    pub fn command_failed(msg: impl Into<String>) -> Self {
        Self::CommandFailed(msg.into())
    }

    pub fn invalid_arguments(msg: impl Into<String>) -> Self {
        Self::InvalidArguments(msg.into())
    }

    pub fn pattern_error(msg: impl Into<String>) -> Self {
        Self::PatternError(msg.into())
    }

    pub fn http_error(msg: impl Into<String>) -> Self {
        Self::Http(msg.into())
    }

    pub fn invalid_url(msg: impl Into<String>) -> Self {
        Self::InvalidUrl(msg.into())
    }

    pub fn network_error(msg: impl Into<String>) -> Self {
        Self::Network(msg.into())
    }
}
