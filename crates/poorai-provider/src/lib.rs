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
    #[error("provider context limit exceeded: {safe_context}")]
    ContextLimit { safe_context: String },
    #[error("provider operation cancelled")]
    Cancelled,
    /// The reply stopped without the backend saying it was finished.
    ///
    /// A short answer and an abandoned one look identical in the assembled
    /// text, so this is the difference between a deployment that answered
    /// briefly and a connection that died mid-generation. Treating the second
    /// as the first is how a truncated reply becomes a recorded result.
    #[error("provider reply was truncated: {safe_context}")]
    Truncated { safe_context: String },
    /// The backend could not parse what the model produced.
    ///
    /// Distinct from `Protocol`, which is the transport or the backend
    /// misbehaving. This is the deployment emitting a tool call the backend's
    /// own template parser rejects -- measured: `XML syntax error on line 3:
    /// unexpected end element </function>` returned in a 200 body.
    ///
    /// It is the same class of thing as a malformed tool call, and belongs in
    /// the same place: told to the deployment and retried under the same
    /// bound, not ending a sixty-action run over one bad generation.
    #[error("deployment produced output the backend could not parse: {safe_context}")]
    ModelOutput { safe_context: String },
}

/// A handle that stops a reply in progress.
///
/// Cancellation was claimed and never demonstrated: `ProviderError::Cancelled`
/// was not constructed anywhere, and the capability probe judged it by reading
/// three chunks, dropping the stream, and calling `/api/ps` to see whether the
/// backend answered. A backend that answers is not a backend that stopped
/// generating.
///
/// What actually stops a local backend is the connection closing, so that is
/// the mechanism here rather than a message: cancelling drops the underlying
/// stream, which drops the HTTP body, which closes the socket. The error is
/// then reported as `Cancelled` rather than as a broken stream, so abandoning a
/// reply is never recorded as the deployment failing.
#[derive(Clone, Default)]
pub struct Cancel {
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    notify: std::sync::Arc<tokio::sync::Notify>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stops the reply this handle guards. Idempotent.
    pub fn cancel(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Resolves once cancelled, and immediately if it already was.
    pub async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            self.notify.notified().await;
        }
    }
}

/// Wraps a stream so cancelling the handle closes it.
///
/// The drop is the point. A flag that only stops the reader leaves the backend
/// generating into a socket nobody reads, which is the resource this is meant
/// to release.
pub fn cancellable(cancel: Cancel, stream: ModelStream) -> ModelStream {
    Box::pin(futures_util::stream::unfold(
        (Some(stream), cancel),
        |(stream, cancel)| async move {
            let mut stream = stream?;
            if cancel.is_cancelled() {
                // Dropped by leaving scope with `None` as the next state; the
                // connection closing is what stops the backend.
                return Some((Err(ProviderError::Cancelled), (None, cancel)));
            }
            tokio::select! {
                next = stream.next() => next.map(|item| (item, (Some(stream), cancel))),
                () = cancel.cancelled() => {
                    drop(stream);
                    Some((Err(ProviderError::Cancelled), (None, cancel)))
                }
            }
        },
    ))
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
    let mut done = false;
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
        if chunk.done {
            done = true;
            break;
        }
        if reply.chunks >= MAX_REPLY_CHUNKS {
            return Err(ProviderError::Truncated {
                safe_context: "reply exceeded the chunk bound before the backend finished".into(),
            });
        }
    }
    if reply.chunks == 0 {
        return Err(ProviderError::Protocol {
            safe_context: "provider returned an empty stream".into(),
        });
    }
    if !done {
        // The stream ended without a terminal chunk. What was assembled may be
        // a whole answer or the first half of one, and nothing here can tell
        // the two apart -- so it is not returned as an answer.
        return Err(ProviderError::Truncated {
            safe_context: "stream ended without a terminal chunk".into(),
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

    /// The same reply, abandonable.
    ///
    /// Defaulted rather than required because the mechanism is transport-level
    /// and the same for every provider that streams over a connection: closing
    /// it is what stops the backend. A provider whose backend needs telling
    /// explicitly overrides this.
    async fn chat_cancellable(
        &self,
        request: ModelRequest,
        cancel: Cancel,
    ) -> Result<ModelStream, ProviderError> {
        Ok(cancellable(cancel, self.chat(request).await?))
    }
}
