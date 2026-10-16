use thiserror::Error;

#[derive(Error, Debug)]
pub enum PageclawError {
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
}

impl PageclawError {
    /// Maps internal error types to standardized process exit codes for AI Agent orchestration.
    pub fn exit_code(&self) -> i32 {
        match self {
            PageclawError::InvalidArgument(_) | PageclawError::UrlParse(_) => 2,
            PageclawError::Network(_) | PageclawError::ConnectionTimeout(_) => 3,
            PageclawError::HttpStatusError { status, .. } if *status >= 500 => 3,
            PageclawError::HttpStatusError { status, .. } if *status >= 400 => 4,
            PageclawError::HttpStatusError { .. } => 1,
            PageclawError::CleaningError(_)
            | PageclawError::ConversionError(_)
            | PageclawError::EncodingError(_) => 4,
            PageclawError::ResponseTooLarge { .. } => 5,
            PageclawError::SsrfBlocked(_) => 6,
            PageclawError::Io(_)
            | PageclawError::AdapterError { .. }
            | PageclawError::CacheError(_)
            | PageclawError::McpError { .. } => 1,
        }
    }
}
