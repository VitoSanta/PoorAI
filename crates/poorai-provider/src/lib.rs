//! Provider-neutral asynchronous model boundary.
use async_trait::async_trait;
use poorai_domain::{
    BackendState, DeploymentDescriptor, ModelChunk, ModelInspection, ModelRequest,
};
use std::pin::Pin;

pub type ModelStream =
    Pin<Box<dyn futures_core::Stream<Item = Result<ModelChunk, ProviderError>> + Send>>;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider unavailable: {safe_context}")]
    Unavailable { safe_context: String },
    #[error("provider protocol error: {safe_context}")]
    Protocol { safe_context: String },
    #[error("provider operation timed out: {safe_context}")]
    Timeout { safe_context: String },
    #[error("provider operation cancelled")]
    Cancelled,
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn inspect(
        &self,
        deployment: &DeploymentDescriptor,
    ) -> Result<ModelInspection, ProviderError>;
    async fn runtime_state(&self) -> Result<BackendState, ProviderError>;
    async fn chat(&self, request: ModelRequest) -> Result<ModelStream, ProviderError>;
}
