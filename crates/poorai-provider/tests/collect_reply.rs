//! A reply is the whole stream, not its first chunk.

use poorai_domain::{GenerationMetrics, ModelChunk, ToolCall};
use poorai_provider::{ModelStream, ProviderError, collect_reply};

fn stream(chunks: Vec<Result<ModelChunk, ProviderError>>) -> ModelStream {
    Box::pin(futures_util::stream::iter(chunks))
}
fn thinking(text: &str) -> ModelChunk {
    ModelChunk {
        thinking: Some(text.into()),
        ..Default::default()
    }
}
fn content(text: &str) -> ModelChunk {
    ModelChunk {
        content: text.into(),
        ..Default::default()
    }
}

/// The defect this exists to prevent, found three times: in the capability
/// probe, in calibration, and in the action loop.
#[tokio::test]
async fn an_answer_after_leading_thinking_chunks_is_assembled() {
    let reply = collect_reply(stream(vec![
        Ok(thinking("The user")),
        Ok(thinking(" wants")),
        Ok(content(r#"{"capability":"#)),
        Ok(content(r#""list_tree","max_entries":10}"#)),
        Ok(ModelChunk {
            done: true,
            ..Default::default()
        }),
    ]))
    .await
    .unwrap();
    assert_eq!(
        reply.content,
        r#"{"capability":"list_tree","max_entries":10}"#
    );
    // Reasoning stays out of the answer channel; a parser must never see it.
    assert_eq!(reply.thinking, "The user wants");
    assert_eq!(reply.chunks, 5);
}

#[tokio::test]
async fn tool_calls_are_collected_from_wherever_they_arrive() {
    let call = |name: &str| ModelChunk {
        tool_calls: vec![ToolCall {
            name: name.into(),
            arguments: serde_json::json!({}),
            id: None,
        }],
        ..Default::default()
    };
    let reply = collect_reply(stream(vec![
        Ok(thinking("hm")),
        Ok(call("first")),
        Ok(content("text")),
        Ok(call("second")),
        Ok(ModelChunk {
            done: true,
            ..Default::default()
        }),
    ]))
    .await
    .unwrap();
    assert_eq!(
        reply
            .tool_calls
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}

#[tokio::test]
async fn terminal_metrics_are_kept() {
    let reply = collect_reply(stream(vec![
        Ok(content("x")),
        Ok(ModelChunk {
            metrics: Some(GenerationMetrics {
                generated_tokens: Some(42),
                generation_duration_ns: Some(1_000_000_000),
                ..Default::default()
            }),
            done: true,
            ..Default::default()
        }),
    ]))
    .await
    .unwrap();
    assert_eq!(reply.metrics.unwrap().tokens_per_second(), Some(42.0));
}

#[tokio::test]
async fn the_stream_stops_at_done() {
    let reply = collect_reply(stream(vec![
        Ok(content("kept")),
        Ok(ModelChunk {
            content: "also kept".into(),
            done: true,
            ..Default::default()
        }),
        Ok(content("after done")),
    ]))
    .await
    .unwrap();
    assert_eq!(reply.content, "keptalso kept");
    assert_eq!(reply.chunks, 2);
}

#[tokio::test]
async fn an_empty_stream_is_an_error_rather_than_an_empty_reply() {
    // An empty answer must never be handed to a parser as if it were one.
    assert!(collect_reply(stream(vec![])).await.is_err());
}

#[tokio::test]
async fn a_stream_error_propagates() {
    let failed = collect_reply(stream(vec![
        Ok(content("partial")),
        Err(ProviderError::Protocol {
            safe_context: "truncated".into(),
        }),
    ]))
    .await;
    assert!(failed.is_err());
}

/// A short answer and an abandoned one assemble into the same text. The only
/// thing that separates them is the terminal chunk, so its absence is a
/// failure rather than the end of a reply.
#[tokio::test]
async fn a_stream_that_ends_without_a_terminal_chunk_is_truncated() {
    let failed = collect_reply(stream(vec![Ok(content("half an ans")), Ok(content("wer"))])).await;
    assert!(matches!(failed, Err(ProviderError::Truncated { .. })));
}

#[tokio::test]
async fn hitting_the_chunk_bound_is_an_error_rather_than_a_short_reply() {
    // A deployment that never stops emitting is bounded, and what was read up
    // to the bound is not an answer: returning it would report a fragment as a
    // complete reply and let it be parsed as an action.
    let chunks: Vec<_> = std::iter::repeat_with(|| Ok(content("x")))
        .take(poorai_provider::MAX_REPLY_CHUNKS + 1)
        .collect();
    let failed = collect_reply(stream(chunks)).await;
    assert!(matches!(failed, Err(ProviderError::Truncated { .. })));
}
