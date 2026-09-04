//! Ollama HTTP adapter. Native DTOs do not escape this crate.
use async_trait::async_trait;
use chrono::Utc;
use futures_util::{StreamExt, stream};
use poorai_domain::{
    BackendState, DeploymentDescriptor, GenerationMetrics, ModelChunk, ModelDefinition,
    ModelInspection, ModelRequest, Observation, Provenance, ToolCall, new_id,
};
use poorai_provider::{ModelProvider, ModelStream, ProviderError};
use reqwest::{Client, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

/// A backend address, carrying whether reaching off this machine was granted.
///
/// "Local" was a default rather than a guarantee: the endpoint accepted any
/// HTTP(S) URL, so a prompt and the repository excerpts inside it could be sent
/// to another host without anything asking. The grant travels with the address
/// instead of being a flag somewhere above, because every constructor below
/// then has to have been given it.
#[derive(Clone, Debug)]
pub struct BackendEndpoint {
    url: Url,
    remote_approved: bool,
}

impl BackendEndpoint {
    /// The default: refuses any address that is not this machine.
    pub fn local(raw: &str) -> Result<Self, ProviderError> {
        let url = Self::parse(raw)?;
        if !Self::is_loopback(&url) {
            return Err(ProviderError::Protocol {
                safe_context: format!(
                    "{} is not a local backend; pass --allow-remote-endpoint to send prompts and repository contents off this machine",
                    url.host_str().unwrap_or("the endpoint")
                ),
            });
        }
        Ok(Self {
            url,
            remote_approved: false,
        })
    }

    /// A remote backend the operator named explicitly.
    pub fn remote_approved(raw: &str) -> Result<Self, ProviderError> {
        Ok(Self {
            url: Self::parse(raw)?,
            remote_approved: true,
        })
    }

    pub fn is_remote(&self) -> bool {
        self.remote_approved && !Self::is_loopback(&self.url)
    }

    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    fn parse(raw: &str) -> Result<Url, ProviderError> {
        Url::parse(raw).map_err(|_| ProviderError::Protocol {
            safe_context: "invalid Ollama endpoint".into(),
        })
    }

    fn is_loopback(url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        if host.eq_ignore_ascii_case("localhost") || host.eq_ignore_ascii_case("localhost.") {
            return true;
        }
        // An IPv6 host arrives bracketed, and the whole 127.0.0.0/8 block is
        // loopback -- not only 127.0.0.1.
        let host = host.trim_start_matches('[').trim_end_matches(']');
        host.parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
    }
}

#[derive(Clone)]
pub struct OllamaProvider {
    client: Client,
    endpoint: Url,
}
impl OllamaProvider {
    pub fn new(endpoint: &BackendEndpoint, timeout: Duration) -> Result<Self, ProviderError> {
        let client = Client::builder()
            .timeout(timeout)
            // A redirect can change host after the address was judged, which
            // would make that judgement advisory rather than binding.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ProviderError::Unavailable {
                safe_context: "could not configure local HTTP client".into(),
            })?;
        Ok(Self {
            client,
            endpoint: endpoint.url.clone(),
        })
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
            let status = response.status();
            let mut stream = response.bytes_stream();
            let mut body = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| ProviderError::Protocol {
                    safe_context: format!("Ollama returned HTTP {status}"),
                })?;
                let remaining = 8192usize.saturating_sub(body.len());
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                if body.len() == 8192 {
                    break;
                }
            }
            let body = String::from_utf8_lossy(&body);
            return Err(classify_backend_error(
                &body,
                format!("Ollama returned HTTP {status}"),
            ));
        }
        Ok(response)
    }

    async fn json_bounded<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
        context: &'static str,
    ) -> Result<T, ProviderError> {
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                if error.is_timeout() {
                    ProviderError::Timeout {
                        safe_context: context.into(),
                    }
                } else {
                    ProviderError::Protocol {
                        safe_context: format!("unreadable {context}"),
                    }
                }
            })?;
            if body.len().saturating_add(chunk.len()) > MAX_JSON_BODY_BYTES {
                return Err(ProviderError::Protocol {
                    safe_context: format!("{context} exceeded the response size limit"),
                });
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| ProviderError::Protocol {
            safe_context: format!("malformed {context}"),
        })
    }
}

const MAX_JSON_BODY_BYTES: usize = 64 * 1024 * 1024;
const MAX_STREAM_LINE_BYTES: usize = 1024 * 1024;

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
    /// Bytes resident on the accelerator. Equal to `size` when the whole model
    /// is there; smaller means part of it is being served from the CPU, which
    /// changes what a latency measurement is measuring.
    #[serde(default)]
    size_vram: Option<u64>,
    #[serde(default)]
    context_length: Option<u64>,
}
#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [poorai_domain::ChatMessage],
    stream: bool,
    options: BTreeMap<&'a str, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
}
#[derive(Deserialize)]
struct ChatResponse {
    /// Ollama reports some failures as an error field in a 200 body rather
    /// than as a status. Without this the chunk deserialises into a reply with
    /// no content, and a backend failure becomes a valid empty answer.
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<NativeMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
    #[serde(default)]
    total_duration: Option<u64>,
    #[serde(default)]
    load_duration: Option<u64>,
    #[serde(default)]
    prompt_eval_duration: Option<u64>,
    #[serde(default)]
    eval_duration: Option<u64>,
}
impl ChatResponse {
    /// Ollama reports counts and timings on the terminal chunk only.
    fn metrics(&self) -> Option<GenerationMetrics> {
        let metrics = GenerationMetrics {
            prompt_tokens: self.prompt_eval_count,
            generated_tokens: self.eval_count,
            total_duration_ns: self.total_duration,
            load_duration_ns: self.load_duration,
            prompt_eval_duration_ns: self.prompt_eval_duration,
            generation_duration_ns: self.eval_duration,
        };
        (metrics != GenerationMetrics::default()).then_some(metrics)
    }
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

/// Reads a backend failure message and says which kind of failure it is.
///
/// The same message arrives two ways -- as a non-2xx body, and as an `error`
/// field inside a 200 -- so the classification lives in one place rather than
/// being right in whichever path was written first.
fn classify_backend_error(message: &str, fallback: String) -> ProviderError {
    let lowered = message.to_ascii_lowercase();
    if ["context length", "input length", "num_ctx", "too long"]
        .iter()
        .any(|needle| lowered.contains(needle))
    {
        return ProviderError::ContextLimit {
            safe_context: format!(
                "Ollama rejected the measured context tier: {}",
                elide(message)
            ),
        };
    }
    // A parse failure of what the model produced, rather than of the
    // transport. Ollama's own template parser rejecting a tool call is the
    // deployment writing badly, not the backend misbehaving, and it belongs
    // where a malformed call belongs.
    if [
        "syntax error",
        "unexpected end element",
        "error parsing tool",
        "invalid tool call",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
    {
        return ProviderError::ModelOutput {
            safe_context: elide(message),
        };
    }
    // The backend's own words, carried rather than discarded. Classifying an
    // error and then throwing away what it said leaves a run that failed for a
    // knowable reason reported as "a protocol error" -- which is the shape of
    // failure this project spent a day removing everywhere else. What is
    // redacted here is prompts and file contents, not diagnostics.
    ProviderError::Protocol {
        safe_context: format!("{fallback}: {}", elide(message)),
    }
}

/// Bounds a backend message without hiding what it says.
fn elide(message: &str) -> String {
    let message = message.trim().replace('\n', " ");
    if message.chars().count() <= MAX_BACKEND_MESSAGE_CHARS {
        return message;
    }
    format!(
        "{}…",
        message
            .chars()
            .take(MAX_BACKEND_MESSAGE_CHARS - 1)
            .collect::<String>()
    )
}

const MAX_BACKEND_MESSAGE_CHARS: usize = 400;

pub fn parse_ndjson_chunks(body: &str) -> Result<Vec<ModelChunk>, ProviderError> {
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let response: ChatResponse =
                serde_json::from_str(line).map_err(|_| ProviderError::Protocol {
                    safe_context: "malformed Ollama streaming chunk".into(),
                })?;
            if let Some(error) = &response.error {
                return Err(classify_backend_error(
                    error,
                    "Ollama reported an error in place of a reply".into(),
                ));
            }
            let metrics = response.metrics();
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
                metrics,
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
        let response = self
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
            .await?;
        let response: ShowResponse = self.json_bounded(response, "Ollama show response").await?;
        let tags_response = self
            .response(self.client.get(self.url("api/tags")?).send().await)
            .await?;
        let tags: TagsResponse = self
            .json_bounded(tags_response, "Ollama tags response")
            .await?;
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
        let response = self
            .response(self.client.get(self.url("api/ps")?).send().await)
            .await?;
        let tags: TagsResponse = self
            .json_bounded(response, "Ollama runtime response")
            .await?;
        // Residency is recorded rather than assumed: a deployment partly served
        // from the CPU is a different machine from one wholly on the
        // accelerator, and a calibration that cannot tell them apart is
        // measuring two things under one name.
        let resident: Vec<serde_json::Value> = tags
            .models
            .iter()
            .map(|m| {
                serde_json::json!({
                    "name": m.name,
                    "size_bytes": m.size,
                    "vram_bytes": m.size_vram,
                    "context_length": m.context_length,
                    "fully_on_accelerator": match (m.size, m.size_vram) {
                        (Some(size), Some(vram)) => Some(vram >= size),
                        // Absent fields are unknown, not false.
                        _ => None,
                    },
                })
            })
            .collect();
        Ok(BackendState {
            observed_at: Utc::now(),
            loaded_models: tags.models.iter().map(|m| m.name.clone()).collect(),
            state: serde_json::json!({
                "source": "ollama:/api/ps",
                "loaded": resident,
                "unknown_fields": "not inferred",
            }),
        })
    }
    async fn chat(&self, request: ModelRequest) -> Result<ModelStream, ProviderError> {
        let mut options = BTreeMap::new();
        options.insert("num_ctx", serde_json::json!(request.context_tokens));
        if let Some(seed) = request.seed {
            options.insert("seed", serde_json::json!(seed));
        }
        // Sent verbatim, so what a report records and what the backend received
        // are the same thing.
        let mut think = None;
        for (name, value) in &request.sampling {
            if name == "think" {
                think = value.as_bool();
            } else {
                options.insert(name.as_str(), value.clone());
            }
        }
        let body = ChatRequest {
            model: &request.deployment.model_ref,
            messages: &request.messages,
            stream: true,
            options,
            tools: request.tools.as_ref(),
            think,
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
            (body, Vec::<u8>::new(), false),
            |(mut body, mut buffer, finished)| async move {
                if finished {
                    return None;
                }
                loop {
                    if let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
                        let mut line: Vec<u8> = buffer.drain(..=index).collect();
                        line.pop();
                        if line.iter().all(u8::is_ascii_whitespace) {
                            continue;
                        }
                        let item = std::str::from_utf8(&line)
                            .map_err(|_| ProviderError::Protocol {
                                safe_context: "Ollama streaming chunk was not UTF-8".into(),
                            })
                            .and_then(parse_ndjson_chunks)
                            .and_then(|mut items| {
                                items.pop().ok_or(ProviderError::Protocol {
                                    safe_context: "empty Ollama streaming chunk".into(),
                                })
                            });
                        return Some((item, (body, buffer, false)));
                    }
                    match body.next().await {
                        Some(Ok(bytes)) => {
                            if buffer.len().saturating_add(bytes.len()) > MAX_STREAM_LINE_BYTES {
                                return Some((
                                    Err(ProviderError::Protocol {
                                        safe_context:
                                            "Ollama streaming line exceeded the size limit".into(),
                                    }),
                                    (body, Vec::new(), true),
                                ));
                            }
                            buffer.extend_from_slice(&bytes);
                        }
                        Some(Err(error)) => {
                            // A client timeout surfaces as a broken body, not
                            // as a timeout error. Reporting it as a protocol
                            // fault blames the backend for a deployment that
                            // was simply too slow to answer within the bound.
                            let failure = if error.is_timeout() {
                                ProviderError::Timeout {
                                    safe_context: "streaming response exceeded the client timeout"
                                        .into(),
                                }
                            } else {
                                ProviderError::Protocol {
                                    safe_context: "unreadable Ollama streaming response".into(),
                                }
                            };
                            return Some((Err(failure), (body, buffer, true)));
                        }
                        None => {
                            if buffer.iter().all(u8::is_ascii_whitespace) {
                                return None;
                            }
                            let item = std::str::from_utf8(&buffer)
                                .map_err(|_| ProviderError::Protocol {
                                    safe_context: "Ollama streaming response was not UTF-8".into(),
                                })
                                .and_then(parse_ndjson_chunks)
                                .and_then(|mut items| {
                                    items.pop().ok_or(ProviderError::Protocol {
                                        safe_context: "empty Ollama streaming response".into(),
                                    })
                                });
                            return Some((item, (body, Vec::new(), true)));
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
    /// Every fixture server binds loopback, which is what the default allows.
    fn local(raw: &str) -> BackendEndpoint {
        BackendEndpoint::local(raw).expect("fixture endpoints are loopback")
    }

    #[test]
    fn rejects_non_url() {
        assert!(BackendEndpoint::local("bad").is_err());
    }

    /// The default is not merely loopback-shaped: an address that leaves this
    /// machine is refused until someone says otherwise, because a prompt
    /// carries repository contents with it.
    #[test]
    fn a_non_local_endpoint_is_refused_unless_it_was_approved() {
        assert!(BackendEndpoint::local("http://198.51.100.7:11434/").is_err());
        assert!(BackendEndpoint::local("https://ollama.example.com/").is_err());
        assert!(BackendEndpoint::remote_approved("https://ollama.example.com/").is_ok());
        assert!(
            BackendEndpoint::remote_approved("https://ollama.example.com/")
                .unwrap()
                .is_remote()
        );
    }

    #[test]
    fn the_whole_loopback_block_counts_as_local() {
        for raw in [
            "http://127.0.0.1:11434/",
            "http://127.0.0.7:11434/",
            "http://localhost:11434/",
            "http://LocalHost:11434/",
            "http://[::1]:11434/",
        ] {
            assert!(BackendEndpoint::local(raw).is_ok(), "{raw} is loopback");
        }
        // A name that merely starts with the word is another host entirely.
        assert!(BackendEndpoint::local("http://localhost.evil.example/").is_err());
    }
    /// Ollama answers some failures with HTTP 200 and an error field. Without
    /// the field on the DTO the chunk deserialises into a message with no
    /// content, and a backend failure reaches the loop as a valid empty reply.
    #[test]
    fn an_error_field_in_a_200_body_is_a_failure_not_an_empty_reply() {
        let failed = parse_ndjson_chunks(r#"{"error":"model requires more system memory"}"#);
        assert!(matches!(failed, Err(ProviderError::Protocol { .. })));
    }

    /// Classifying an error and discarding what it said leaves a run that
    /// failed for a knowable reason reported as "a protocol error". Found by a
    /// real run ending on one, with nothing to diagnose it by.
    #[test]
    fn a_backend_error_carries_what_the_backend_said() {
        let failed = parse_ndjson_chunks(
            r#"{"error":"model requires more system memory (32.0 GiB) than is available"}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(failed.contains("more system memory"), "{failed}");

        // Bounded, and a newline does not break the line it is reported on.
        let long = "x".repeat(5_000);
        let failed = parse_ndjson_chunks(&format!(
            r#"{{"error":"a
b {long}"}}"#
        ))
        .unwrap_err()
        .to_string();
        assert!(failed.len() < 600, "unbounded: {} chars", failed.len());
        assert!(!failed.contains('\n'));
    }

    #[test]
    fn an_error_field_naming_the_context_is_classified_as_a_context_limit() {
        // The same message arrives as a non-2xx body and inside a 200. A tier
        // downgrade must be reachable from both, or the retry depends on which
        // shape the backend happened to use.
        let failed =
            parse_ndjson_chunks(r#"{"error":"input length 5000 exceeds context length 4096"}"#);
        assert!(matches!(failed, Err(ProviderError::ContextLimit { .. })));
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
        let provider = OllamaProvider::new(&local(&endpoint), Duration::from_secs(2)).unwrap();
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
        let provider = OllamaProvider::new(&local(&endpoint), Duration::from_secs(2)).unwrap();
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
        let provider = OllamaProvider::new(&local(&endpoint), Duration::from_secs(2)).unwrap();
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
        let provider = OllamaProvider::new(&local(&endpoint), Duration::from_secs(2)).unwrap();
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
        let provider = OllamaProvider::new(
            &local(&format!("http://{address}/")),
            Duration::from_millis(10),
        )
        .unwrap();
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
        let provider = OllamaProvider::new(
            &local(&format!("http://{address}/")),
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(matches!(
            provider.runtime_state().await,
            Err(ProviderError::Protocol { .. })
        ));
    }

    #[tokio::test]
    async fn runtime_state_rejects_malformed_json() {
        let endpoint = fixture_server(vec!["{not-json"]);
        let provider = OllamaProvider::new(&local(&endpoint), Duration::from_secs(1)).unwrap();
        assert!(matches!(
            provider.runtime_state().await,
            Err(ProviderError::Protocol { .. })
        ));
    }

    #[tokio::test]
    async fn chat_rejects_an_unbounded_ndjson_line() {
        let endpoint = fixture_server_owned(vec!["x".repeat(MAX_STREAM_LINE_BYTES + 1)]);
        let provider = OllamaProvider::new(&local(&endpoint), Duration::from_secs(2)).unwrap();
        let deployment = DeploymentDescriptor {
            schema_version: 1,
            id: poorai_domain::new_id(),
            provider: "ollama".into(),
            endpoint,
            model_ref: "fixture".into(),
            backend_options: Default::default(),
            auth_ref: None,
        };
        let mut stream = provider
            .chat(poorai_domain::ModelRequest {
                deployment,
                context_tokens: 32,
                tools: None,
                seed: None,
                sampling: Default::default(),
                messages: vec![poorai_domain::ChatMessage {
                    role: "user".into(),
                    content: "hello".into(),
                    ..Default::default()
                }],
            })
            .await
            .unwrap();
        assert!(matches!(
            stream.next().await,
            Some(Err(ProviderError::Protocol { .. }))
        ));
    }
    /// Cancellation was claimed and never demonstrated. The probe read three
    /// chunks, dropped the stream, and asked `/api/ps` whether the backend was
    /// alive -- but a backend that answers is not a backend that stopped
    /// generating, and `ProviderError::Cancelled` was never constructed.
    ///
    /// What stops a local backend is the connection closing, so that is what is
    /// asserted: the fixture generates without end and reports the moment its
    /// writes start failing, which is the socket going away.
    #[tokio::test]
    async fn cancelling_closes_the_connection_the_backend_is_writing_to() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<usize>();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\n\r\n",
            );
            // Generates without end, the way a deployment mid-reply does.
            let chunk = "{\"message\":{\"content\":\"x\"},\"done\":false}\n";
            let framed = format!("{:x}\r\n{chunk}\r\n", chunk.len());
            let _ = stream.set_read_timeout(Some(Duration::from_millis(5)));
            for written in 0..20_000usize {
                if stream.write_all(framed.as_bytes()).is_err() || stream.flush().is_err() {
                    let _ = tx.send(written);
                    return;
                }
                // A closed peer shows as end-of-file on the read side, which
                // is the earliest and least ambiguous signal that the client
                // is gone -- a write to a closed socket can succeed for a
                // while into the kernel's buffer.
                let mut probe = [0u8; 1];
                if let Ok(0) = stream.read(&mut probe) {
                    let _ = tx.send(written);
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let _ = tx.send(usize::MAX);
        });

        let endpoint = format!("http://{address}/");
        let provider = OllamaProvider::new(&local(&endpoint), Duration::from_secs(30)).unwrap();
        let deployment = DeploymentDescriptor {
            schema_version: 1,
            id: poorai_domain::new_id(),
            provider: "ollama".into(),
            endpoint,
            model_ref: "fixture".into(),
            backend_options: Default::default(),
            auth_ref: None,
        };
        let cancel = poorai_provider::Cancel::new();
        let mut stream = provider
            .chat_cancellable(
                poorai_domain::ModelRequest {
                    deployment,
                    context_tokens: 32,
                    tools: None,
                    seed: None,
                    sampling: Default::default(),
                    messages: vec![poorai_domain::ChatMessage {
                        role: "user".into(),
                        content: "hello".into(),
                        ..Default::default()
                    }],
                },
                cancel.clone(),
            )
            .await
            .unwrap();
        // Read a little, so the reply is genuinely underway.
        for _ in 0..3 {
            assert!(matches!(stream.next().await, Some(Ok(_))));
        }
        cancel.cancel();
        // Abandoning reports itself as cancellation rather than as the stream
        // simply ending, so an abandoned reply is never recorded as the
        // deployment failing.
        assert!(matches!(
            stream.next().await,
            Some(Err(ProviderError::Cancelled))
        ));
        drop(stream);

        // Awaited rather than blocked on. The connection is closed by a task
        // the runtime owns, so blocking this thread on the channel stops the
        // very thing being asserted -- which is how this fixture first failed
        // against correct code.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let written = loop {
            match rx.try_recv() {
                Ok(written) => break written,
                Err(_) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(_) => panic!("the backend never saw the connection close"),
            }
        };
        assert!(
            written != usize::MAX,
            "the backend was still generating into the socket after cancellation"
        );
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
        let provider = OllamaProvider::new(&local(&endpoint), Duration::from_secs(1)).unwrap();
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
            seed: None,
            sampling: Default::default(),
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
