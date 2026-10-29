use thiserror::Error;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ErrorDetails {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

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

    #[error("Batch checkpoint error: {0}")]
    CheckpointError(String),

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

    #[error("Search anti-scraping challenge: {0}")]
    SearchAntiScraping(String),

    #[error("PDF extraction error: {0}")]
    PdfError(String),

    #[error("Search engine error: {0}")]
    SearchEngineError(String),

    #[error("Unrecognized Bing results page: {0}")]
    SearchParseError(String),

    #[error("Search connection failed: {0}")]
    SearchConnectionError(String),

    #[error("Search rate limited for {engine}; retry after {retry_after_secs} seconds")]
    SearchRateLimited {
        engine: String,
        retry_after_secs: u64,
    },
}

impl TklessError {
    /// Stable error metadata shared by library and MCP callers.
    pub fn details(&self) -> ErrorDetails {
        let code = match self {
            Self::Network(_) => "network_error",
            Self::UrlParse(_) => "invalid_url",
            Self::ResponseTooLarge { .. } => "response_too_large",
            Self::EncodingError(_) => "encoding_error",
            Self::CleaningError(_) => "cleaning_error",
            Self::ConversionError(_) => "conversion_error",
            Self::Io(_) => "io_error",
            Self::InvalidArgument(_) => "invalid_argument",
            Self::AdapterError { .. } => "adapter_error",
            Self::SsrfBlocked(_) => "ssrf_blocked",
            Self::CacheError(_) => "cache_error",
            Self::CheckpointError(_) => "checkpoint_error",
            Self::McpError { .. } => "protocol_error",
            Self::HttpStatusError { .. } => "http_error",
            Self::ConnectionTimeout(_) => "timeout",
            Self::CdpError(_) => "browser_error",
            Self::SchemaError(_) => "schema_error",
            Self::SearchAntiScraping(_) => "verification_required",
            Self::PdfError(_) => "pdf_error",
            Self::SearchEngineError(_) => "search_error",
            Self::SearchParseError(_) => "search_parse_error",
            Self::SearchConnectionError(_) => "search_connection_error",
            Self::SearchRateLimited { .. } => "rate_limited",
        };
        ErrorDetails {
            code: code.into(),
            message: self.to_string(),
            exit_code: self.exit_code(),
            retryable: matches!(
                self,
                Self::Network(_)
                    | Self::ConnectionTimeout(_)
                    | Self::SearchConnectionError(_)
                    | Self::SearchRateLimited { .. }
            ) || matches!(self, Self::HttpStatusError { status, .. } if *status >= 500 || *status == 429),
            retry_after_secs: match self {
                Self::SearchRateLimited {
                    retry_after_secs, ..
                } => Some(*retry_after_secs),
                _ => None,
            },
        }
    }

    /// Maps internal error types to standardized process exit codes for AI Agent orchestration.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::InvalidArgument(_) | Self::UrlParse(_) => 2,
            Self::Network(_)
            | Self::ConnectionTimeout(_)
            | Self::SearchEngineError(_)
            | Self::SearchConnectionError(_)
            | Self::SearchRateLimited { .. } => 3,
            Self::HttpStatusError { status, .. } if *status >= 500 => 3,
            Self::HttpStatusError { status, .. } if *status >= 400 => 4,
            Self::HttpStatusError { .. } => 1,
            Self::CleaningError(_)
            | Self::ConversionError(_)
            | Self::EncodingError(_)
            | Self::PdfError(_) => 4,
            Self::SearchParseError(_) => 4,
            Self::ResponseTooLarge { .. } => 5,
            Self::SsrfBlocked(_) => 6,
            Self::Io(_)
            | Self::AdapterError { .. }
            | Self::CacheError(_)
            | Self::CheckpointError(_)
            | Self::CdpError(_)
            | Self::SchemaError(_)
            | Self::SearchAntiScraping(_)
            | Self::McpError { .. } => 1,
        }
    }
}
