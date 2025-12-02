//! Common utilities for rust-analyzer tools

use crate::rust_analyzer::RustAnalyzerClient;
use lazy_static::lazy_static;
use std::sync::Arc;
use tokio::sync::RwLock;

lazy_static! {
    /// Global rust-analyzer client instance
    /// Wrapped in Arc for cheap cloning across tools
    pub static ref RUST_ANALYZER: Arc<RwLock<Option<Arc<RustAnalyzerClient>>>> = Arc::new(RwLock::new(None));
}

/// Set the global rust-analyzer client
pub async fn set_client(client: RustAnalyzerClient) {
    *RUST_ANALYZER.write().await = Some(Arc::new(client));
}

/// Get a cloned reference to the global rust-analyzer client
pub async fn get_client() -> Result<Arc<RustAnalyzerClient>, crate::error::ToolError> {
    let guard = RUST_ANALYZER.read().await;
    guard.as_ref()
        .map(Arc::clone)
        .ok_or_else(|| {
            crate::error::ToolError::Other(
                "rust-analyzer is not available (not a Rust project or rust-analyzer not installed)".to_string()
            )
        })
}

/// Check if rust-analyzer is available
pub async fn is_available() -> bool {
    RUST_ANALYZER.read().await.is_some()
}
