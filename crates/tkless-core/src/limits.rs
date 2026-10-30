//! Shared workflow limits used by core validation, CLI parsing and MCP schemas.
pub const COLLECTION_DEFAULT_PAGES: usize = 10;
pub const COLLECTION_MAX_PAGES: usize = 1000;
pub const COLLECTION_DEFAULT_ITEMS: usize = 1000;
pub const COLLECTION_MAX_ITEMS: usize = 100_000;
pub const COLLECTION_MAX_DELAY_MS: u64 = 60_000;
/// Upper bound for a templated request body, measured after placeholder expansion.
pub const COLLECTION_MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const COLLECTION_MAX_REQUEST_HEADERS: usize = 16;
pub const COLLECTION_MAX_REQUEST_HEADER_BYTES: usize = 8 * 1024;
pub const COLLECTION_DEFAULT_PAGE_SIZE: u64 = 20;
pub const COLLECTION_MAX_PAGE_SIZE: u64 = 1000;
pub const SYNC_DEFAULT_STATE_MB: u64 = 256;
pub const SYNC_MAX_STATE_MB: u64 = 65_536;
pub const MIB: u64 = 1024 * 1024;
pub const BROWSER_DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const BROWSER_MAX_TIMEOUT_MS: u64 = 120_000;
pub const BROWSER_DEFAULT_WAIT_MS: u64 = 250;
pub const BROWSER_MAX_WAIT_MS: u64 = 30_000;
pub const BROWSER_MAX_STEPS: usize = 32;
pub const BROWSER_MAX_SELECTOR_BYTES: usize = 1024;
pub const BROWSER_MAX_FILL_BYTES: usize = 16_384;
pub const BROWSER_MAX_SCROLL_PIXELS: u32 = 10_000;
pub const BROWSER_SCROLL_MAX_STEPS: u32 = 60;
pub const BROWSER_SCROLL_DEFAULT_STEPS: u32 = 20;
pub const BROWSER_SCROLL_DEFAULT_IDLE_ROUNDS: u32 = 2;
pub const BROWSER_SCROLL_MAX_IDLE_ROUNDS: u32 = 10;
/// Captured network responses are opt-in and bounded per page.
pub const BROWSER_CAPTURE_DEFAULT_RESPONSES: usize = 20;
pub const BROWSER_CAPTURE_MAX_RESPONSES: usize = 50;
pub const BROWSER_CAPTURE_DEFAULT_BODY_BYTES: usize = 256 * 1024;
pub const BROWSER_CAPTURE_MAX_BODY_BYTES: usize = 1024 * 1024;
pub const BROWSER_CAPTURE_MIN_BODY_BYTES: usize = 1024;
pub const BROWSER_CAPTURE_MAX_MIME_PREFIXES: usize = 8;
pub const BROWSER_MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

pub fn sync_state_bytes(megabytes: u64) -> Result<u64, crate::TklessError> {
    if !(1..=SYNC_MAX_STATE_MB).contains(&megabytes) {
        return Err(crate::TklessError::InvalidArgument(format!(
            "State budget must be 1..{SYNC_MAX_STATE_MB} MiB"
        )));
    }
    Ok(megabytes * MIB)
}
