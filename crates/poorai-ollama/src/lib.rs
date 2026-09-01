//! Ollama HTTP adapter. Native DTOs do not escape this crate.
use async_trait::async_trait;
use chrono::Utc;
use futures_util::{StreamExt, stream};
use poorai_domain::{
    BackendState, DeploymentDescriptor, ModelChunk, ModelDefinition, ModelInspection, ModelRequest,
    Observation, Provenance, ToolCall, new_id,
};
use poorai_provider::{ModelProvider, ModelStream, ProviderError};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone)]
pub struct OllamaProvider {
    client: Client,
    endpoint: Url,
}
impl OllamaProvider {
    pub fn new(endpoint: &str, timeout: Duration) -> Result<Self, ProviderError> {
        let endpoint = Url::parse(endpoint).map_err(|_| ProviderError::Protocol {
            safe_context: "invalid Ollama endpoint".into(),
        })?;
        let client =
            Client::builder()
                .timeout(timeout)
                .build()
                .map_err(|_| ProviderError::Unavailable {
                    safe_context: "could not configure local HTTP client".into(),
                })?;
        Ok(Self { client, endpoint })
    }
    fn url(&self, path: &str) -> Result<Url, ProviderError> {
        self.endpoint
            .join(path)
            .map_err(|_| ProviderError::Protocol {
                safe_context: "invalid Ollama route".into(),
            })
    }
    async fn response(
        &self,
        response: reqwest::Result<reqwest::Response>,
    ) -> Result<reqwest::Response, ProviderError> {
        let response = response.map_err(|e| {
            if e.is_timeout() {
                ProviderError::Timeout {
                    safe_context: "local Ollama request".into(),
                }
            } else {
                ProviderError::Unavailable {
                    safe_context: "local Ollama request".into(),
                }
            }
        })?;
        if !response.status().is_success() {
            return Err(ProviderError::Protocol {
                safe_context: format!("Ollama returned HTTP {}", response.status()),
            });
        }
        Ok(response)
    }
}

#[derive(Serialize)]
struct ShowRequest<'a> {
    name: &'a str,
    verbose: bool,
}
#[derive(Deserialize)]
struct ShowResponse {
    #[serde(default)]
    details: Details,
    #[serde(default)]
    model_info: serde_json::Value,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    modified_at: Option<String>,
}
#[derive(Default, Deserialize)]
struct Details {
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    quantization_level: Option<String>,
    #[serde(default)]
    parent_model: Option<String>,
}
#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagModel>,
}
#[derive(Deserialize)]
struct TagModel {
    name: String,
    #[serde(default)]
    digest: String,
    #[serde(default)]
    size: Option<u64>,
}
#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [poorai_domain::ChatMessage],
    stream: bool,
    options: BTreeMap<&'a str, u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a serde_json::Value>,
}
#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    message: Option<NativeMessage>,
    #[serde(default)]
    done: bool,
}
#[derive(Default, Deserialize)]
struct NativeMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    tool_calls: Vec<NativeToolCall>,
}
#[derive(Deserialize)]
struct NativeToolCall {
    #[serde(default)]
    id: Option<String>,
    function: NativeToolFunction,
}
#[derive(Deserialize)]
struct NativeToolFunction {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}
/// Serving metadata may embed the full tokenizer vocabulary (`tokenizer.ggml.tokens`,
/// `merges`, `scores`, `token_type`) — tens of megabytes per model that are not
/// serving facts and would bloat every persisted `ModelDefinition`.
///
/// Replace any oversized array with its length and a content hash, so the
/// observation stays auditable evidence rather than a silent omission.
const MAX_INLINE_ARRAY_LEN: usize = 64;

fn prune_bulk_arrays(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) if items.len() > MAX_INLINE_ARRAY_LEN => {
            let encoded = serde_json::to_vec(&items).unwrap_or_default();
            serde_json::json!({
                "pruned": "oversized_array",
                "length": items.len(),
                "content_hash": poorai_domain::hash_bytes(&encoded),
            })
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(prune_bulk_arrays).collect())
        }
        serde_json::Value::Object(fields) => serde_json::Value::Object(
            fields
                .into_iter()
                .map(|(key, nested)| (key, prune_bulk_arrays(nested)))
                .collect(),
        ),
        scalar => scalar,
    }
}

pub fn parse_ndjson_chunks(body: &str) -> Result<Vec<ModelChunk>, ProviderError> {
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let response: ChatResponse =
                serde_json::from_str(line).map_err(|_| ProviderError::Protocol {
                    safe_context: "malformed Ollama streaming chunk".into(),
                })?;
            let message = response.message.unwrap_or_default();
            Ok(ModelChunk {
                content: message.content,
                thinking: message.thinking.filter(|t| !t.is_empty()),
                tool_calls: message
                    .tool_calls
                    .into_iter()
                    .map(|call| ToolCall {
                        name: call.function.name,
                        arguments: call.function.arguments,
                        id: call.id,
                    })
                    .collect(),
                done: response.done,
            })
        })
        .collect()
}

#[async_trait]
impl ModelProvider for OllamaProvider {
    async fn inspect(
        &self,
        deployment: &DeploymentDescriptor,
    ) -> Result<ModelInspection, ProviderError> {
        let response: ShowResponse = self
            .response(
                self.client
                    .post(self.url("api/show")?)
                    .json(&ShowRequest {
                        name: &deployment.model_ref,
                        verbose: true,
                    })
                    .send()
                    .await,
            )
            .await?
            .json()
            .await
            .map_err(|_| ProviderError::Protocol {
                safe_context: "malformed Ollama show response".into(),
            })?;
        let tags: TagsResponse = self
            .response(self.client.get(self.url("api/tags")?).send().await)
            .await?
            .json()
            .await
            .map_err(|_| ProviderError::Protocol {
                safe_context: "malformed Ollama tags response".into(),
            })?;
        let tag = tags.models.iter().find(|m| m.name == deployment.model_ref);
        let digest = tag
            .map(|m| m.digest.clone())
            .filter(|x| !x.is_empty())
            .ok_or_else(|| ProviderError::Protocol {
                safe_context: "model digest unavailable from Ollama tags".into(),
            })?;
        let mut capabilities = BTreeMap::new();
        for capability in response.capabilities {
            capabilities.insert(
                capability,
                Observation::Observed(serde_json::Value::Bool(true)),
            );
        }
        if capabilities.is_empty() {
            capabilities.insert(
                "capability_probe".into(),
                Observation::Unknown {
                    reason: "Ollama response omitted capabilities; active probe required".into(),
                },
            );
        }
        let metadata = serde_json::json!({ "model_info": prune_bulk_arrays(response.model_info), "reported_modified_at": response.modified_at, "reported_size_bytes": tag.and_then(|m| m.size), "parent_model": response.details.parent_model });
        Ok(ModelInspection {
            definition: ModelDefinition {
                schema_version: 1,
                id: new_id(),
                digest,
                family: response.details.family,
                quantization: response.details.quantization_level,
                capabilities,
                metadata,
                provenance: Provenance {
                    source: "ollama:/api/show,/api/tags".into(),
                    observed_at: Utc::now(),
                    content_hash: poorai_domain::hash_bytes(deployment.model_ref.as_bytes()),
                },
            },
            deployment: deployment.clone(),
        })
    }
    async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
        let tags: TagsResponse = self
            .response(self.client.get(self.url("api/ps")?).send().await)
            .await?
            .json()
            .await
            .map_err(|_| ProviderError::Protocol {
                safe_context: "malformed Ollama runtime response".into(),
            })?;
        Ok(BackendState {
            observed_at: Utc::now(),
            loaded_models: tags.models.into_iter().map(|m| m.name).collect(),
            state: serde_json::json!({"source":"ollama:/api/ps", "unknown_fields":"not inferred"}),
        })
    }
    async fn chat(&self, request: ModelRequest) -> Result<ModelStream, ProviderError> {
        let mut options = BTreeMap::new();
        options.insert("num_ctx", request.context_tokens);
        let body = ChatRequest {
            model: &request.deployment.model_ref,
            messages: &request.messages,
            stream: true,
            options,
            tools: request.tools.as_ref(),
        };
        let response = self
            .response(
                self.client
                    .post(self.url("api/chat")?)
                    .json(&body)
                    .send()
                    .await,
            )
            .await?;
        let body = response.bytes_stream();
        let parsed = stream::unfold(
            (body, String::new(), false),
            |(mut body, mut buffer, finished)| async move {
                if finished {
                    return None;
                }
                loop {
                    if let Some(index) = buffer.find('\n') {
                        let line = buffer[..index].to_string();
                        buffer = buffer[index + 1..].to_string();
                        if line.trim().is_empty() {
                            continue;
                        }
                        let item = parse_ndjson_chunks(&line).and_then(|mut items| {
                            items.pop().ok_or(ProviderError::Protocol {
                                safe_context: "empty Ollama streaming chunk".into(),
                            })
                        });
                        return Some((item, (body, buffer, false)));
                    }
                    match body.next().await {
                        Some(Ok(bytes)) => buffer.push_str(&String::from_utf8_lossy(&bytes)),
                        Some(Err(_)) => {
                            return Some((
                                Err(ProviderError::Protocol {
                                    safe_context: "unreadable Ollama streaming response".into(),
                                }),
                                (body, buffer, true),
                            ));
                        }
                        None => {
                            if buffer.trim().is_empty() {
                                return None;
                            }
                            let item = parse_ndjson_chunks(&buffer).and_then(|mut items| {
                                items.pop().ok_or(ProviderError::Protocol {
                                    safe_context: "empty Ollama streaming response".into(),
                                })
                            });
                            return Some((item, (body, String::new(), true)));
                        }
                    }
                }
            },
        );
        Ok(Box::pin(parsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    fn fixture_server(responses: Vec<&'static str>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for body in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        format!("http://{address}/")
    }
    fn fixture_server_owned(responses: Vec<String>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for body in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        format!("http://{address}/")
    }
    #[test]
    fn rejects_non_url() {
        assert!(OllamaProvider::new("bad", Duration::from_secs(1)).is_err());
    }
    #[test]
    fn native_dto_defaults_do_not_infer_capabilities() {
        let parsed: ShowResponse =
            serde_json::from_str(r#"{"details":{},"model_info":{}}"#).unwrap();
        assert!(parsed.capabilities.is_empty());
        assert!(parsed.details.family.is_none());
    }
    #[test]
    fn tool_call_is_parsed_structurally_and_not_flattened_into_content() {
        let chunk = &parse_ndjson_chunks(
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"id":"call_1","function":{"index":0,"name":"probe_echo","arguments":{"value":"ok"}}}]},"done":false}"#,
        )
        .unwrap()[0];
        assert_eq!(chunk.tool_calls.len(), 1);
        assert_eq!(chunk.tool_calls[0].name, "probe_echo");
        assert_eq!(chunk.tool_calls[0].arguments["value"], "ok");
        assert_eq!(chunk.tool_calls[0].id.as_deref(), Some("call_1"));
        // The call must not leak into the prose channel: callers read the
        // typed field, never re-parse `content` to guess a call happened.
        assert!(chunk.content.is_empty());
    }

    #[test]
    fn reasoning_chunk_carries_thinking_without_content() {
        let chunk = &parse_ndjson_chunks(
            r#"{"message":{"role":"assistant","content":"","thinking":"The"},"done":false}"#,
        )
        .unwrap()[0];
        assert_eq!(chunk.thinking.as_deref(), Some("The"));
        assert!(chunk.content.is_empty());
        assert!(chunk.tool_calls.is_empty());
        // This shape is exactly the opening chunk of a reasoning deployment.
        // It carries no tool evidence, so a first-chunk verdict would be wrong.
        assert!(!chunk.is_empty());
    }

    #[test]
    fn empty_thinking_is_not_recorded_as_present() {
        let chunk = &parse_ndjson_chunks(
            r#"{"message":{"role":"assistant","content":"","thinking":""},"done":true}"#,
        )
        .unwrap()[0];
        assert!(chunk.thinking.is_none());
        assert!(chunk.is_empty());
    }

    #[test]
    fn reasoning_stream_delivers_late_tool_call_across_chunks() {
        // A tool call arriving after leading thinking chunks must survive the
        // NDJSON parse in order, so a draining caller can still observe it.
        let chunks = parse_ndjson_chunks(concat!(
            r#"{"message":{"role":"assistant","content":"","thinking":"The"},"done":false}"#,
            "\n",
            r#"{"message":{"role":"assistant","content":"","thinking":" user"},"done":false}"#,
            "\n",
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"probe_echo","arguments":{"value":"ok"}}}]},"done":false}"#,
            "\n",
            r#"{"message":{"role":"assistant","content":""},"done":true}"#,
        ))
        .unwrap();
        assert_eq!(chunks.len(), 4);
        assert!(chunks[0].tool_calls.is_empty());
        let calls: Vec<_> = chunks.iter().filter(|c| !c.tool_calls.is_empty()).collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_calls[0].name, "probe_echo");
        assert!(chunks[3].done);
    }

    #[test]
    fn oversized_metadata_arrays_are_replaced_by_length_and_hash() {
        let tokens: Vec<serde_json::Value> = (0..5000)
            .map(|i| serde_json::json!(format!("t{i}")))
            .collect();
        let pruned = prune_bulk_arrays(serde_json::json!({
            "tokenizer": {"ggml": {"tokens": tokens, "bos_token_id": 1}},
            "general": {"architecture": "test"},
        }));
        let entry = &pruned["tokenizer"]["ggml"]["tokens"];
        assert_eq!(entry["pruned"], "oversized_array");
        assert_eq!(entry["length"], 5000);
        assert!(
            entry["content_hash"]
                .as_str()
                .is_some_and(|h| !h.is_empty())
        );
        // Scalars and small values are untouched serving facts.
        assert_eq!(pruned["tokenizer"]["ggml"]["bos_token_id"], 1);
        assert_eq!(pruned["general"]["architecture"], "test");
    }

    #[test]
    fn small_metadata_arrays_are_preserved_verbatim() {
        let value = serde_json::json!({"eos_token_ids": [1, 2, 3]});
        assert_eq!(prune_bulk_arrays(value.clone()), value);
    }

    #[tokio::test]
    async fn inspect_prunes_tokenizer_vocabulary_from_persisted_facts() {
        let tokens: String = (0..5000)
            .map(|i| format!("\"t{i}\""))
            .collect::<Vec<_>>()
            .join(",");
        let show = format!(
            r#"{{"details":{{"family":"test"}},"capabilities":["completion"],"model_info":{{"tokenizer.ggml.tokens":[{tokens}]}}}}"#
        );
        let endpoint = fixture_server_owned(vec![
            show,
            r#"{"models":[{"name":"fixture","digest":"sha256:abc"}]}"#.to_string(),
        ]);
        let provider = OllamaProvider::new(&endpoint, Duration::from_secs(2)).unwrap();
        let deployment = DeploymentDescriptor {
            schema_version: 1,
            id: poorai_domain::new_id(),
            provider: "ollama".into(),
            endpoint,
            model_ref: "fixture".into(),
            backend_options: Default::default(),
            auth_ref: None,
        };
        let result = provider.inspect(&deployment).await.unwrap();
        let entry = &result.definition.metadata["model_info"]["tokenizer.ggml.tokens"];
        assert_eq!(entry["length"], 5000);
        assert!(serde_json::to_vec(&result.definition).unwrap().len() < 4096);
    }

    #[test]
    fn malformed_chat_dto_has_no_message() {
        let parsed: ChatResponse = serde_json::from_str(r#"{"done":true}"#).unwrap();
        assert!(parsed.message.is_none());
        assert!(parsed.done);
    }
    #[tokio::test]
    async fn inspect_maps_local_fixture_to_domain_facts() {
        let endpoint = fixture_server(vec![
            r#"{"details":{"family":"test"},"capabilities":["completion"],"model_info":{}}"#,
            r#"{"models":[{"name":"fixture","digest":"sha256:abc"}]}"#,
        ]);
        let provider = OllamaProvider::new(&endpoint, Duration::from_secs(2)).unwrap();
        let deployment = DeploymentDescriptor {
            schema_version: 1,
            id: poorai_domain::new_id(),
            provider: "ollama".into(),
            endpoint,
            model_ref: "fixture".into(),
            backend_options: Default::default(),
            auth_ref: None,
        };
        let result = provider.inspect(&deployment).await.unwrap();
        assert_eq!(result.definition.digest, "sha256:abc");
        assert_eq!(result.definition.family.as_deref(), Some("test"));
    }
    #[tokio::test]
    async fn inspect_rejects_fixture_without_digest() {
        let endpoint = fixture_server(vec![
            r#"{"details":{},"model_info":{}}"#,
            r#"{"models":[{"name":"fixture"}]}"#,
        ]);
        let provider = OllamaProvider::new(&endpoint, Duration::from_secs(2)).unwrap();
        let deployment = DeploymentDescriptor {
            schema_version: 1,
            id: poorai_domain::new_id(),
            provider: "ollama".into(),
            endpoint,
            model_ref: "fixture".into(),
            backend_options: Default::default(),
            auth_ref: None,
        };
        assert!(matches!(
            provider.inspect(&deployment).await,
            Err(ProviderError::Protocol { .. })
        ));
    }
    #[tokio::test]
    async fn inspect_rejects_malformed_show_json() {
        let endpoint = fixture_server(vec!["{invalid"]);
        let provider = OllamaProvider::new(&endpoint, Duration::from_secs(2)).unwrap();
        let deployment = DeploymentDescriptor {
            schema_version: 1,
            id: poorai_domain::new_id(),
            provider: "ollama".into(),
            endpoint,
            model_ref: "fixture".into(),
            backend_options: Default::default(),
            auth_ref: None,
        };
        assert!(matches!(
            provider.inspect(&deployment).await,
            Err(ProviderError::Protocol { .. })
        ));
    }
    #[tokio::test]
    async fn runtime_state_maps_slow_fixture_to_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(100));
        });
        let provider =
            OllamaProvider::new(&format!("http://{address}/"), Duration::from_millis(10)).unwrap();
        assert!(matches!(
            provider.runtime_state().await,
            Err(ProviderError::Timeout { .. })
        ));
    }
    #[tokio::test]
    async fn runtime_state_maps_non_success_status_to_protocol_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let provider =
            OllamaProvider::new(&format!("http://{address}/"), Duration::from_secs(1)).unwrap();
        assert!(matches!(
            provider.runtime_state().await,
            Err(ProviderError::Protocol { .. })
        ));
    }
    #[tokio::test]
    async fn chat_emits_first_ndjson_chunk_before_body_finishes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let first = "{\"message\":{\"content\":\"O\"},\"done\":false}\n";
            let second = "{\"message\":{\"content\":\"K\"},\"done\":true}\n";
            let header = "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            stream.write_all(header.as_bytes()).unwrap();
            stream
                .write_all(format!("{:X}\r\n{}\r\n", first.len(), first).as_bytes())
                .unwrap();
            stream.flush().unwrap();
            std::thread::sleep(Duration::from_millis(80));
            stream
                .write_all(format!("{:X}\r\n{}\r\n0\r\n\r\n", second.len(), second).as_bytes())
                .unwrap();
        });
        let endpoint = format!("http://{address}/");
        let provider = OllamaProvider::new(&endpoint, Duration::from_secs(1)).unwrap();
        let deployment = DeploymentDescriptor {
            schema_version: 1,
            id: poorai_domain::new_id(),
            provider: "ollama".into(),
            endpoint,
            model_ref: "fixture".into(),
            backend_options: Default::default(),
            auth_ref: None,
        };
        let request = poorai_domain::ModelRequest {
            deployment,
            context_tokens: 32,
            tools: None,
            messages: vec![],
        };
        let mut chunks = provider.chat(request).await.unwrap();
        let first = tokio::time::timeout(Duration::from_millis(40), chunks.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(first.content, "O");
        let second = chunks.next().await.unwrap().unwrap();
        assert_eq!(second.content, "K");
        assert!(second.done);
    }
    #[test]
    fn ndjson_parser_preserves_stream_chunks() {
        let chunks=parse_ndjson_chunks("{\"message\":{\"content\":\"O\"},\"done\":false}\n{\"message\":{\"content\":\"K\"},\"done\":true}\n").unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].content, "K");
        assert!(chunks[1].done);
    }
    #[test]
    fn ndjson_parser_rejects_malformed_chunk() {
        assert!(parse_ndjson_chunks("not-json\n").is_err());
    }
}
