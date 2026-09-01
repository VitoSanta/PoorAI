//! Provider-neutral asynchronous model boundary.
use async_trait::async_trait;
use futures_util::StreamExt;
use poorai_domain::{
    BackendState, DeploymentDescriptor, GenerationMetrics, ModelChunk, ModelInspection,
    ModelRequest, ToolCall,
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

/// One model reply, assembled from every chunk of its stream.
///
/// Reading a stream's first chunk is not reading a reply: a reasoning
/// deployment opens with `thinking` chunks whose content is empty, and its
/// answer and any tool call arrive later -- as late as the final chunk. Every
/// consumer goes through here so that mistake has one place to not be made.
#[derive(Debug, Clone, Default)]
pub struct ModelReply {
    /// Assembled answer text, excluding the reasoning channel.
    pub content: String,
    pub thinking: String,
    pub tool_calls: Vec<ToolCall>,
    pub chunks: usize,
    pub metrics: Option<GenerationMetrics>,
}

/// Upper bound on chunks read from one reply, guarding against a deployment
/// that never stops emitting. Wall clock is bounded by the provider timeout.
pub const MAX_REPLY_CHUNKS: usize = 16_384;

/// Reads a stream to completion and returns the assembled reply.
pub async fn collect_reply(mut stream: ModelStream) -> Result<ModelReply, ProviderError> {
    let mut reply = ModelReply::default();
    while let Some(next) = stream.next().await {
        let chunk = next?;
        reply.chunks += 1;
        reply.content.push_str(&chunk.content);
        if let Some(thinking) = &chunk.thinking {
            reply.thinking.push_str(thinking);
        }
        reply.tool_calls.extend(chunk.tool_calls);
        if chunk.metrics.is_some() {
            reply.metrics = chunk.metrics;
        }
        if chunk.done || reply.chunks >= MAX_REPLY_CHUNKS {
            break;
        }
    }
    if reply.chunks == 0 {
        return Err(ProviderError::Protocol {
            safe_context: "provider returned an empty stream".into(),
        });
    }
    Ok(reply)
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
