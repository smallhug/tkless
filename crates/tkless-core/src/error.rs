use thiserror::Error;

#[derive(Error, Debug)]
pub enum TklessError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("URL parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("Response too large: received {received} bytes, maximum allowed is {limit} bytes")]
    ResponseTooLarge { limit: usize, received: usize },

    #[error("Encoding decode error: {0}")]
    EncodingError(String),

    #[error("Content cleaning error: {0}")]
    CleaningError(String),

    #[error("Markdown conversion error: {0}")]
    ConversionError(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Platform adapter error ({platform}): {message}")]
    AdapterError {
        platform: &'static str,
        message: String,
    },

    #[error("SSRF blocked: access to private/loopback/cloud-metadata address is restricted: {0}")]
    SsrfBlocked(String),

    #[error("Cache error: {0}")]
    CacheError(String),

    #[error("MCP protocol error ({code}): {message}")]
    McpError { code: i32, message: String },

    #[error("HTTP error status {status} for {url}")]
    HttpStatusError { status: u16, url: String },

    #[error("Connection timed out: {0}")]
    ConnectionTimeout(String),

    #[error("CDP browser error: {0}")]
    CdpError(String),

    #[error("Schema extraction error: {0}")]
    SchemaError(String),
}

impl TklessError {
    /// Maps internal error types to standardized process exit codes for AI Agent orchestration.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::InvalidArgument(_) | Self::UrlParse(_) => 2,
            Self::Network(_) | Self::ConnectionTimeout(_) => 3,
            Self::HttpStatusError { status, .. } if *status >= 500 => 3,
            Self::HttpStatusError { status, .. } if *status >= 400 => 4,
            Self::HttpStatusError { .. } => 1,
            Self::CleaningError(_) | Self::ConversionError(_) | Self::EncodingError(_) => 4,
            Self::ResponseTooLarge { .. } => 5,
            Self::SsrfBlocked(_) => 6,
            Self::Io(_)
            | Self::AdapterError { .. }
            | Self::CacheError(_)
            | Self::CdpError(_)
            | Self::SchemaError(_)
            | Self::McpError { .. } => 1,
        }
    }
}

/// Backward-compatible type alias for `TklessError`.
pub type PageclawError = TklessError;
