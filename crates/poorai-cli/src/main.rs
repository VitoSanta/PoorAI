use clap::{Args, Parser, Subcommand};
use futures_util::StreamExt;
use poorai_domain::{
    ChatMessage, DeploymentDescriptor, HardwareProfile, ModelRequest, Observation, Provenance,
    ToolCall, hash_bytes, new_id, now,
};
use poorai_ollama::OllamaProvider;
use poorai_provider::ModelProvider;
use serde::Serialize;
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(name = "poorai", about = "Local, evidence-driven coding agent")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true, default_value = "http://127.0.0.1:11434/")]
    ollama_endpoint: String,
}
#[derive(Subcommand)]
enum Command {
    Doctor,
    Models(Models),
    Repo(Repo),
    Verify {
        run_id: Option<String>,
        #[arg(long, default_value = "targeted")]
        scope: String,
    },
    Calibrate {
        model: String,
        #[arg(long, value_delimiter = ',', default_value = "2048,4096,8192")]
        ladder: Vec<u32>,
        /// Seeds the tier-order shuffle. Recorded with the profile so the run
        /// is reproducible.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Free-memory percentage below which a sample counts as under
        /// pressure. A declared policy floor, recorded with every reading.
        #[arg(long, default_value_t = 20)]
        pressure_floor: u8,
        /// Fraction of samples a tier must pass to be a stable point.
        #[arg(long, default_value_t = 1.0)]
        min_success_rate: f64,
        /// Median first-token latency a tier may not exceed.
        #[arg(long, default_value_t = 120_000.0)]
        max_median_first_token_ms: f64,
    },
    Run {
        task: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        profile: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    Eval(Eval),
    Report {
        id: String,
        #[arg(long, default_value = "json")]
        format: String,
    },
}
#[derive(Args)]
struct Models {
    #[command(subcommand)]
    command: ModelsCommand,
}
#[derive(Subcommand)]
enum ModelsCommand {
    Inspect {
        model: String,
        #[arg(long)]
        probe: bool,
        /// Provider timeout for this inspection. A cold 30B deployment can take
        /// minutes to load before its first token, and a load that outruns the
        /// timeout is recorded as `unknown`, not as a missing capability.
        #[arg(long, default_value_t = 300)]
        timeout_secs: u64,
        /// Repetitions of each non-deterministic capability trial. Tool-call
        /// emission is sampled behaviour, so a single trial cannot distinguish
        /// "unsupported" from "did not happen this time".
        #[arg(long, default_value_t = 3)]
        probe_trials: u32,
    },
}
#[derive(Args)]
struct Repo {
    #[command(subcommand)]
    command: RepoCommand,
}
#[derive(Subcommand)]
enum RepoCommand {
    Index { path: Option<PathBuf> },
}
#[derive(Args)]
struct Eval {
    #[command(subcommand)]
    command: EvalCommand,
}
#[derive(Subcommand)]
enum EvalCommand {
    Run {
        suite: String,
        #[arg(long)]
        model: String,
    },
}
#[derive(Serialize)]
struct Output<T: Serialize> {
    schema_version: u32,
    ok: bool,
    result: Option<T>,
    error: Option<SafeError>,
}
#[derive(Serialize)]
struct SafeError {
    category: &'static str,
    context: String,
}
fn print<T: Serialize>(json: bool, result: Result<T, SafeError>) -> i32 {
    match result {
        Ok(value) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&Output {
                        schema_version: 1,
                        ok: true,
                        result: Some(value),
                        error: None
                    })
                    .unwrap()
                );
            } else {
                println!("{}", serde_json::to_string_pretty(&value).unwrap());
            }
            0
        }
        Err(error) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&Output::<serde_json::Value> {
                        schema_version: 1,
                        ok: false,
                        result: None,
                        error: Some(error)
                    })
                    .unwrap()
                );
            } else {
                eprintln!("{}: {}", error.category, error.context);
            }
            4
        }
    }
}
#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Doctor => print(cli.json, doctor(&cli.ollama_endpoint).await),
        Command::Models(m) => match m.command {
            ModelsCommand::Inspect {
                model,
                probe,
                timeout_secs,
                probe_trials,
            } => print(
                cli.json,
                inspect(
                    &cli.ollama_endpoint,
                    model,
                    probe,
                    Duration::from_secs(timeout_secs),
                    probe_trials.max(1),
                )
                .await,
            ),
        },
        Command::Repo(r) => match r.command {
            RepoCommand::Index { path } => print(
                cli.json,
                index_repository(path.unwrap_or_else(|| PathBuf::from("."))),
            ),
        },
        Command::Calibrate {
            model,
            ladder,
            seed,
            pressure_floor,
            min_success_rate,
            max_median_first_token_ms,
        } => print(
            cli.json,
            calibrate(
                &cli.ollama_endpoint,
                model,
                ladder,
                seed,
                pressure_floor,
                min_success_rate,
                max_median_first_token_ms,
            )
            .await,
        ),
        Command::Run {
            task,
            model,
            profile,
            dry_run,
        } => print(
            cli.json,
            run(task, model, profile, dry_run, &cli.ollama_endpoint).await,
        ),
        Command::Verify { run_id, scope } => print(cli.json, verify(run_id, scope).await),
        Command::Eval(e) => match e.command {
            EvalCommand::Run { suite, model } => print(
                cli.json,
                Ok(
                    serde_json::json!({"suite":suite,"model":model,"status":"requires a frozen corpus"}),
                ),
            ),
        },
        Command::Report { id, format } => print(cli.json, report(id, format)),
    };
    std::process::exit(code);
}
async fn doctor(endpoint: &str) -> Result<serde_json::Value, SafeError> {
    let hardware = probe_hardware().await;
    let provider = OllamaProvider::new(endpoint, Duration::from_secs(4)).map_err(provider_error)?;
    let runtime = provider.runtime_state().await.map_err(provider_error);
    Ok(
        serde_json::json!({"hardware":hardware,"ollama_runtime":runtime.map(|x|serde_json::to_value(x).unwrap()).unwrap_or_else(|e|serde_json::json!({"status":"unavailable","reason":e.context})),"facts_only":true}),
    )
}
/// Tool offered by the capability probe. Only a call naming it counts as evidence.
const PROBE_TOOL: &str = "probe_echo";
/// Upper bound on chunks read from one probe. The provider timeout bounds wall
/// clock; this bounds memory against a deployment that never stops emitting.
const PROBE_MAX_CHUNKS: usize = 4096;

#[derive(Debug, Default)]
struct ProbeObservation {
    chunks: usize,
    produced_text: bool,
    tool_call_names: Vec<String>,
    matched_tool_call: Option<ToolCall>,
    matched_chunk_index: Option<usize>,
}

/// Read a probe stream to completion rather than judging it by its first chunk.
///
/// A reasoning deployment opens with `thinking` chunks carrying empty content and
/// emits its tool call near the end of the stream, so a first-chunk verdict
/// reports "no native tool call" for models that in fact make one.
async fn drain_probe(
    stream: Result<poorai_provider::ModelStream, poorai_provider::ProviderError>,
    expect_tool: Option<&str>,
) -> Result<ProbeObservation, String> {
    let mut stream = stream.map_err(|error| format!("probe failed: {error}"))?;
    let mut observed = ProbeObservation::default();
    while let Some(next) = stream.next().await {
        let chunk = match next {
            Ok(chunk) => chunk,
            Err(error) if observed.chunks == 0 => {
                return Err(format!("probe stream error: {error}"));
            }
            // Partial evidence is still evidence: a tool call already seen stands.
            Err(_) => break,
        };
        observed.chunks += 1;
        observed.produced_text |= !chunk.content.is_empty();
        for call in chunk.tool_calls {
            if expect_tool.is_some_and(|name| name == call.name)
                && observed.matched_tool_call.is_none()
            {
                observed.matched_chunk_index = Some(observed.chunks);
                observed.matched_tool_call = Some(call.clone());
            }
            observed.tool_call_names.push(call.name);
        }
        if chunk.done || observed.chunks >= PROBE_MAX_CHUNKS {
            break;
        }
    }
    if observed.chunks == 0 {
        return Err("probe returned no chunk".into());
    }
    Ok(observed)
}

/// Fold repeated tool trials into one observation.
///
/// Emission is sampled behaviour, so the rate is part of the fact: a deployment
/// that answers 1 of 3 must never read the same as one that answers 3 of 3.
/// Zero calls stays `unknown` — absence of a call in n trials is not proof the
/// deployment cannot make one.
fn summarize_tool_trials(
    trials: u32,
    successes: Vec<serde_json::Value>,
    failures: Vec<String>,
) -> Observation {
    if successes.is_empty() {
        return Observation::Unknown {
            reason: format!(
                "no native tool call in {trials} trial(s): {}",
                failures.join("; ")
            ),
        };
    }
    Observation::Observed(serde_json::json!({
        "tool": PROBE_TOOL,
        "trials": trials,
        "calls": successes.len(),
        "reliable": successes.len() == trials as usize,
        "evidence": successes,
        "failures": failures,
    }))
}

/// Prompt used to get a deployment generating long enough to interrupt it.
const CANCEL_PROMPT: &str = "Count slowly from 1 to 500, one number per line. Do not stop early.";
/// Chunks to consume before abandoning the stream. Generation must be genuinely
/// underway, or dropping the stream proves nothing.
const CANCEL_AFTER_CHUNKS: usize = 3;

/// Consume a stream until generation is underway, then abandon it.
///
/// Returns how many chunks were read before the drop. Abandoning the stream is
/// what cancels the request: dropping the response body closes the connection.
async fn abandon_mid_stream(
    stream: Result<poorai_provider::ModelStream, poorai_provider::ProviderError>,
) -> Result<usize, String> {
    let mut stream = stream.map_err(|error| format!("cancellation probe failed: {error}"))?;
    let mut chunks = 0usize;
    while chunks < CANCEL_AFTER_CHUNKS {
        match stream.next().await {
            Some(Ok(chunk)) => {
                chunks += 1;
                // A deployment that finished before we could interrupt it gives
                // no evidence either way.
                if chunk.done {
                    return Err(format!(
                        "stream completed after {chunks} chunk(s) before it could be interrupted"
                    ));
                }
            }
            Some(Err(error)) => return Err(format!("cancellation probe stream error: {error}")),
            None => {
                return Err(format!(
                    "stream ended after {chunks} chunk(s) before it could be interrupted"
                ));
            }
        }
    }
    drop(stream);
    Ok(chunks)
}

/// Observe whether an in-flight generation can be interrupted without wedging
/// the backend.
///
/// Evidence is threefold: generation was genuinely underway, abandoning the
/// stream returned control promptly, and the backend still served a request
/// afterwards. A backend left unresponsive is a failed cancellation, not a
/// successful one.
async fn probe_cancellation(
    provider: &OllamaProvider,
    deployment: &DeploymentDescriptor,
) -> Observation {
    let request = ModelRequest {
        deployment: deployment.clone(),
        context_tokens: 512,
        tools: None,
        messages: vec![ChatMessage {
            role: "user".into(),
            content: CANCEL_PROMPT.into(),
        }],
    };
    let started = std::time::Instant::now();
    let chunks = match abandon_mid_stream(provider.chat(request).await).await {
        Ok(chunks) => chunks,
        Err(reason) => return Observation::Unknown { reason },
    };
    let abandoned_after_ms = started.elapsed().as_millis();
    match provider.runtime_state().await {
        Ok(_) => Observation::Observed(serde_json::json!({
            "chunks_before_cancel": chunks,
            "abandoned_after_ms": abandoned_after_ms,
            "backend_responsive_after": true,
        })),
        Err(error) => Observation::Unknown {
            reason: format!("backend did not answer after cancellation: {error}"),
        },
    }
}

async fn inspect(
    endpoint: &str,
    model: String,
    probe: bool,
    timeout: Duration,
    trials: u32,
) -> Result<serde_json::Value, SafeError> {
    let provider = OllamaProvider::new(endpoint, timeout).map_err(provider_error)?;
    let deployment = DeploymentDescriptor {
        schema_version: 1,
        id: new_id(),
        provider: "ollama".into(),
        endpoint: endpoint.into(),
        model_ref: model,
        backend_options: BTreeMap::new(),
        auth_ref: None,
    };
    let mut inspection = provider
        .inspect(&deployment)
        .await
        .map_err(provider_error)?;
    if probe {
        let request = ModelRequest {
            deployment: deployment.clone(),
            context_tokens: 512,
            tools: None,
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "Reply with OK.".into(),
            }],
        };
        match drain_probe(provider.chat(request).await, None).await {
            Ok(observed) => {
                inspection.definition.capabilities.insert(
                    "chat".into(),
                    Observation::Observed(serde_json::json!({
                        "chunks": observed.chunks,
                        "produced_text": observed.produced_text,
                    })),
                );
                // Streaming is only demonstrated by more than one incremental chunk;
                // a single terminal chunk is a non-streaming reply.
                if observed.chunks > 1 {
                    inspection.definition.capabilities.insert(
                        "streaming".into(),
                        Observation::Observed(serde_json::json!({"chunks": observed.chunks})),
                    );
                } else {
                    inspection.definition.capabilities.insert(
                        "streaming".into(),
                        Observation::Unknown {
                            reason: "deployment returned a single chunk; incremental delivery not demonstrated".into(),
                        },
                    );
                }
            }
            Err(reason) => {
                for key in ["chat", "streaming"] {
                    inspection.definition.capabilities.insert(
                        key.into(),
                        Observation::Unknown {
                            reason: reason.clone(),
                        },
                    );
                }
            }
        }
        let tool_schema = serde_json::json!([{"type":"function","function":{"name":PROBE_TOOL,"description":"Return a probe value without side effects.","parameters":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}}}]);
        // A single sample records a coin flip as a fact: some deployments emit a
        // native call only intermittently. Repeat the trial and report the rate.
        let mut successes: Vec<serde_json::Value> = Vec::new();
        let mut failures: Vec<String> = Vec::new();
        for trial in 1..=trials {
            let tool_request = ModelRequest {
                deployment: deployment.clone(),
                context_tokens: 512,
                tools: Some(tool_schema.clone()),
                messages: vec![ChatMessage {
                    role: "user".into(),
                    content: "Call the probe_echo tool with value 'ok'. Do not answer in prose."
                        .into(),
                }],
            };
            match drain_probe(provider.chat(tool_request).await, Some(PROBE_TOOL)).await {
                // A native call is only credited when the deployment named the
                // tool this probe actually offered.
                Ok(observed) => match observed.matched_tool_call {
                    Some(call) => successes.push(serde_json::json!({
                        "trial": trial,
                        "arguments": call.arguments,
                        "chunk_index": observed.matched_chunk_index,
                        "chunks": observed.chunks,
                    })),
                    None if observed.tool_call_names.is_empty() => {
                        failures.push(format!("trial {trial}: no native tool call"));
                    }
                    None => failures.push(format!(
                        "trial {trial}: called {:?} instead of the offered {PROBE_TOOL} tool",
                        observed.tool_call_names
                    )),
                },
                Err(reason) => failures.push(format!("trial {trial}: {reason}")),
            }
        }
        inspection.definition.capabilities.insert(
            "structured_tools".into(),
            summarize_tool_trials(trials, successes, failures),
        );
        inspection.definition.capabilities.insert(
            "cancellation".into(),
            probe_cancellation(&provider, &deployment).await,
        );
        for (name, reason) in [
            (
                "context_boundary",
                "boundary probe requires calibrated deployment limits",
            ),
            (
                "edit",
                "edit capability is evaluated through the typed-action harness",
            ),
        ] {
            inspection
                .definition
                .capabilities
                .entry(name.into())
                .or_insert_with(|| Observation::Unknown {
                    reason: reason.into(),
                });
        }
    }
    let root = std::env::current_dir()
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?
        .canonicalize()
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    let dir = root.join(".poorai/models");
    std::fs::create_dir_all(&dir).map_err(|e| SafeError {
        category: "internal",
        context: e.to_string(),
    })?;
    let artifact = dir.join(format!("{}.json", inspection.definition.digest));
    std::fs::write(
        &artifact,
        serde_json::to_vec_pretty(&inspection.definition).expect("serializable"),
    )
    .map_err(|e| SafeError {
        category: "internal",
        context: e.to_string(),
    })?;
    Ok(serde_json::json!({"inspection":inspection,"artifact":artifact,"probed":probe}))
}
/// Bump when any measurement step changes; stored profiles invalidate on it.
const CALIBRATION_HARNESS_REV: &str = "calibration-harness-v3";

#[allow(clippy::too_many_arguments)]
async fn calibrate(
    endpoint: &str,
    model: String,
    ladder: Vec<u32>,
    seed: u64,
    pressure_floor: u8,
    min_success_rate: f64,
    max_median_first_token_ms: f64,
) -> Result<serde_json::Value, SafeError> {
    let provider =
        OllamaProvider::new(endpoint, Duration::from_secs(600)).map_err(provider_error)?;
    let deployment = DeploymentDescriptor {
        schema_version: 1,
        id: new_id(),
        provider: "ollama".into(),
        endpoint: endpoint.into(),
        model_ref: model,
        backend_options: BTreeMap::new(),
        auth_ref: None,
    };
    let inspected = provider
        .inspect(&deployment)
        .await
        .map_err(provider_error)?;
    let hardware = probe_hardware().await;
    let host = MacosHostProbe {
        free_percent_floor: pressure_floor,
    };
    let (profile, samples) = poorai_orchestrator::calibrate(
        &provider,
        &host,
        &deployment,
        &hardware,
        inspected.definition.digest,
        &ladder,
        CALIBRATION_HARNESS_REV,
        poorai_domain::CalibrationThresholds {
            min_success_rate,
            max_median_first_token_ms,
            allow_memory_pressure: false,
        },
        seed,
    )
    .await
    .map_err(|e| SafeError {
        category: "calibration",
        context: e,
    })?;
    let root = std::env::current_dir()
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?
        .canonicalize()
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    let dir = root.join(".poorai/calibrations");
    std::fs::create_dir_all(&dir).map_err(|e| SafeError {
        category: "internal",
        context: e.to_string(),
    })?;
    let artifact = dir.join(format!("{}.json", profile.id));
    let temporary = dir.join(format!("{}.tmp", profile.id));
    std::fs::write(
        &temporary,
        // The raw samples travel with the profile: a stable point without the
        // measurements behind it is a claim, not evidence.
        serde_json::to_vec_pretty(&serde_json::json!({
            "profile": profile,
            "seed": seed,
            "samples": samples,
        }))
        .expect("serializable"),
    )
    .map_err(|e| SafeError {
        category: "internal",
        context: e.to_string(),
    })?;
    std::fs::rename(&temporary, &artifact).map_err(|e| SafeError {
        category: "internal",
        context: e.to_string(),
    })?;
    Ok(serde_json::json!({
        "calibration": profile,
        "artifact": artifact,
        "seed": seed,
        "samples": samples,
        "invalidation_keys": ["model_digest","deployment_fingerprint","compatibility_key","harness_rev"],
    }))
}
async fn run(
    task: String,
    model: Option<String>,
    profile: Option<PathBuf>,
    dry_run: bool,
    endpoint: &str,
) -> Result<serde_json::Value, SafeError> {
    if !dry_run {
        return prepare_profiled_run(task, model, profile, endpoint).await;
    }
    let root = std::env::current_dir()
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?
        .canonicalize()
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    let index = poorai_repo::index(&root).map_err(|e| SafeError {
        category: "invalid_input",
        context: e.to_string(),
    })?;
    let plan =
        poorai_orchestrator::prepare_dry_run(task, model, &index).map_err(|e| SafeError {
            category: "invalid_input",
            context: e,
        })?;
    let state_dir = root.join(".poorai");
    std::fs::create_dir_all(&state_dir).map_err(|e| SafeError {
        category: "internal",
        context: e.to_string(),
    })?;
    let index_artifact = poorai_repo::persist(&index, &state_dir).map_err(|e| SafeError {
        category: "internal",
        context: e.to_string(),
    })?;
    let store =
        poorai_store::Store::open(state_dir.join("state.sqlite")).map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    for checkpoint in &plan.checkpoints {
        store
            .append(
                Some(plan.run_id),
                "task.transition",
                serde_json::to_value(checkpoint).expect("serializable"),
            )
            .map_err(|e| SafeError {
                category: "internal",
                context: e.to_string(),
            })?;
    }
    store
        .append(
            Some(plan.run_id),
            "task.plan",
            serde_json::to_value(&plan).expect("serializable"),
        )
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    Ok(serde_json::json!({"plan":plan,"index_artifact":index_artifact}))
}
async fn prepare_profiled_run(
    task: String,
    model: Option<String>,
    profile: Option<PathBuf>,
    endpoint: &str,
) -> Result<serde_json::Value, SafeError> {
    let model = model.ok_or_else(|| SafeError {
        category: "invalid_input",
        context: "--model is required; routing is deferred".into(),
    })?;
    let path = profile.ok_or_else(|| SafeError {
        category: "invalid_input",
        context: "--profile CALIBRATION_JSON is required".into(),
    })?;
    let calibration: poorai_domain::CalibrationProfile =
        serde_json::from_slice(&std::fs::read(&path).map_err(|e| SafeError {
            category: "invalid_input",
            context: e.to_string(),
        })?)
        .map_err(|e| SafeError {
            category: "invalid_input",
            context: format!("invalid calibration profile: {e}"),
        })?;
    let root = std::env::current_dir()
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?
        .canonicalize()
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    let provider =
        OllamaProvider::new(endpoint, Duration::from_secs(20)).map_err(provider_error)?;
    let deployment = DeploymentDescriptor {
        schema_version: 1,
        id: new_id(),
        provider: "ollama".into(),
        endpoint: endpoint.into(),
        model_ref: model,
        backend_options: BTreeMap::new(),
        auth_ref: None,
    };
    let inspection = provider
        .inspect(&deployment)
        .await
        .map_err(provider_error)?;
    let hardware = probe_hardware().await;
    let backend = provider.runtime_state().await.map_err(provider_error)?;
    let runtime = poorai_orchestrator::snapshot(
        &hardware,
        &deployment,
        None,
        probe_memory_pressure().await,
        backend.state,
    );
    let execution = poorai_orchestrator::select_compatible_profile(
        new_id(),
        &calibration,
        &inspection.definition.digest,
        &deployment,
        &hardware,
        "calibration-harness-v1",
    )
    .map_err(|e| SafeError {
        category: "invalid_input",
        context: e,
    })?;
    let dir = root.join(".poorai/execution-profiles");
    std::fs::create_dir_all(&dir).map_err(|e| SafeError {
        category: "internal",
        context: e.to_string(),
    })?;
    let artifact = dir.join(format!("{}.json", execution.id));
    std::fs::write(
        &artifact,
        serde_json::to_vec_pretty(&execution).expect("serializable"),
    )
    .map_err(|e| SafeError {
        category: "internal",
        context: e.to_string(),
    })?;
    let checks = poorai_verify::discover_checks(&root, "targeted").map_err(|e| SafeError {
        category: "invalid_input",
        context: e,
    })?;
    let policy = poorai_tools::ToolPolicy {
        root: root.clone(),
        allow_commands: vec!["cargo".into()],
        output_limit: 64 * 1024,
        timeout: Duration::from_secs(120),
        network_enabled: false,
        sandbox: poorai_tools::SandboxPolicy::Preferred,
        // No approval is granted by default; the CLI has no flag to grant one
        // until a run can actually ask the user for it.
        approvals: Vec::new(),
    };
    let state_dir = root.join(".poorai");
    std::fs::create_dir_all(&state_dir).map_err(|e| SafeError {
        category: "internal",
        context: e.to_string(),
    })?;
    let store =
        poorai_store::Store::open(state_dir.join("state.sqlite")).map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    let request=poorai_domain::ModelRequest { deployment:deployment.clone(),context_tokens:execution.context_tokens,tools:None,messages:vec![poorai_domain::ChatMessage { role:"system".into(),content:"Return exactly one ActionProposal JSON object. Use only read_file, search, list_tree, apply_replace, or run_command.".into() },poorai_domain::ChatMessage { role:"user".into(),content:task.clone() }]};
    let result = poorai_orchestrator::run_action_loop(
        &store,
        &provider,
        request,
        &policy,
        &checks,
        execution.budgets["max_actions"]
            .as_u64()
            .unwrap_or(1)
            .try_into()
            .unwrap_or(1),
    )
    .await
    .map_err(|e| SafeError {
        category: "task_failed",
        context: e,
    })?;
    Ok(
        serde_json::json!({"task":task,"execution_profile":execution,"runtime_snapshot":runtime,"artifact":artifact,"run":result}),
    )
}
fn index_repository(path: PathBuf) -> Result<serde_json::Value, SafeError> {
    let index = poorai_repo::index(path).map_err(|e| SafeError {
        category: "invalid_input",
        context: e.to_string(),
    })?;
    let artifact = poorai_repo::persist(&index, &PathBuf::from(&index.root).join(".poorai"))
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    Ok(serde_json::json!({"index":index,"artifact":artifact,"stale":false}))
}
fn report(id: String, format: String) -> Result<serde_json::Value, SafeError> {
    if format != "json" {
        return Err(SafeError {
            category: "invalid_input",
            context: "only json report format is currently supported".into(),
        });
    }
    let run_id = uuid::Uuid::parse_str(&id).map_err(|_| SafeError {
        category: "invalid_input",
        context: "id must be a UUID".into(),
    })?;
    let root = std::env::current_dir()
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?
        .canonicalize()
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    let store =
        poorai_store::Store::open(root.join(".poorai/state.sqlite")).map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    let events = store.events_for_run(run_id).map_err(|e| SafeError {
        category: "internal",
        context: e.to_string(),
    })?;
    if events.is_empty() {
        return Err(SafeError {
            category: "invalid_input",
            context: "no events found for run".into(),
        });
    }
    Ok(serde_json::json!({"run_id":run_id,"events":events}))
}
async fn verify(run_id: Option<String>, scope: String) -> Result<serde_json::Value, SafeError> {
    let root = std::env::current_dir()
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?
        .canonicalize()
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    let checks = poorai_verify::discover_checks(&root, &scope).map_err(|e| SafeError {
        category: "invalid_input",
        context: e,
    })?;
    let policy = poorai_tools::ToolPolicy {
        root: root.clone(),
        allow_commands: vec!["cargo".into()],
        output_limit: 64 * 1024,
        timeout: Duration::from_secs(120),
        network_enabled: false,
        sandbox: poorai_tools::SandboxPolicy::Preferred,
        // No approval is granted by default; the CLI has no flag to grant one
        // until a run can actually ask the user for it.
        approvals: Vec::new(),
    };
    let state_dir = root.join(".poorai");
    std::fs::create_dir_all(&state_dir).map_err(|e| SafeError {
        category: "internal",
        context: e.to_string(),
    })?;
    let store =
        poorai_store::Store::open(state_dir.join("state.sqlite")).map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    let parsed = run_id
        .as_deref()
        .map(|id| {
            uuid::Uuid::parse_str(id).map_err(|_| SafeError {
                category: "invalid_input",
                context: "run_id must be a UUID".into(),
            })
        })
        .transpose()?;
    let previous = parsed
        .and_then(|id| {
            store
                .latest_payload(id, "verification.baseline")
                .ok()
                .flatten()
        })
        .and_then(|payload| {
            serde_json::from_value::<poorai_verify::VerificationBaseline>(payload).ok()
        });
    let baseline = poorai_verify::baseline(&policy, &checks)
        .await
        .map_err(|e| SafeError {
            category: match e {
                poorai_tools::ToolError::Denied(_) => "policy_denied",
                poorai_tools::ToolError::Timeout => "verification_timeout",
                poorai_tools::ToolError::Io(_) => "verification_io",
            },
            context: e.to_string(),
        })?;
    store
        .append(
            parsed,
            "verification.baseline",
            serde_json::to_value(&baseline).expect("serializable"),
        )
        .map_err(|e| SafeError {
            category: "internal",
            context: e.to_string(),
        })?;
    let passed = baseline
        .checks
        .iter()
        .all(|check| check.result.exit_code == Some(0));
    let comparison = previous
        .as_ref()
        .map(|prior| poorai_verify::compare(prior, &baseline));
    let verified = passed
        && comparison
            .as_ref()
            .is_none_or(|result| result.regression_free);
    Ok(
        serde_json::json!({"verified":verified,"checks_passed":passed,"scope":scope,"workspace_root":root,"baseline":baseline,"comparison":comparison,"note":"verified requires current checks and no baseline regressions when a prior baseline exists"}),
    )
}
fn provider_error(e: poorai_provider::ProviderError) -> SafeError {
    let category = match e {
        poorai_provider::ProviderError::Unavailable { .. }
        | poorai_provider::ProviderError::Timeout { .. } => "provider_unavailable",
        poorai_provider::ProviderError::Protocol { .. } => "provider_protocol",
        poorai_provider::ProviderError::Cancelled => "provider_cancelled",
    };
    SafeError {
        category,
        context: e.to_string(),
    }
}
async fn probe_hardware() -> HardwareProfile {
    let os = run_probe("uname", &["-s"])
        .await
        .unwrap_or_else(|| "unknown".into());
    let architecture = run_probe("uname", &["-m"])
        .await
        .unwrap_or_else(|| "unknown".into());
    let cpu = run_probe("sysctl", &["-n", "machdep.cpu.brand_string"])
        .await
        .unwrap_or_else(|| "unknown".into());
    let total_memory_bytes = run_probe("sysctl", &["-n", "hw.memsize"])
        .await
        .and_then(|x| x.parse().ok());
    let storage_free_bytes = run_probe("df", &["-k", "."]).await.and_then(|x| {
        x.lines()
            .nth(1)
            .and_then(|l| l.split_whitespace().nth(3))
            .and_then(|n| n.parse::<u64>().ok())
            .map(|x| x * 1024)
    });
    let mut unavailable = Vec::new();
    if total_memory_bytes.is_none() {
        unavailable.push("total_memory_bytes".into());
    }
    if storage_free_bytes.is_none() {
        unavailable.push("storage_free_bytes".into());
    }
    HardwareProfile {
        schema_version: 1,
        id: new_id(),
        compatibility_key: hash_bytes(format!(
            "{}|{}|{}|{:?}",
            os, architecture, cpu, total_memory_bytes
        )),
        os,
        architecture,
        cpu,
        accelerators: vec![],
        total_memory_bytes,
        storage_free_bytes,
        unavailable_fields: unavailable,
        probe_version: "macos-command-probe-v1".into(),
        provenance: Provenance {
            source: "uname,sysctl,df (no identifiers)".into(),
            observed_at: now(),
            content_hash: "probe-output-not-persisted".into(),
        },
    }
}
async fn run_probe(command: &str, args: &[&str]) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::process::Command::new(command).args(args).output(),
    )
    .await
    .ok()?
    .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}
/// Host probe backing calibration samples on this machine.
///
/// `free_percent_floor` is a declared policy floor, not an inferred capacity:
/// it decides when an observed free-memory reading counts as pressure, and it
/// is recorded alongside the reading so the judgement can be re-examined.
struct MacosHostProbe {
    free_percent_floor: u8,
}
#[async_trait::async_trait]
impl poorai_orchestrator::HostProbe for MacosHostProbe {
    async fn memory_pressure(&self) -> Observation {
        match probe_memory_pressure().await {
            Observation::Observed(value) => {
                let free = value
                    .get("system_free_percent")
                    .and_then(serde_json::Value::as_u64);
                match free {
                    Some(free) => Observation::Observed(serde_json::json!({
                        "system_free_percent": free,
                        "free_percent_floor": self.free_percent_floor,
                        "under_pressure": free < self.free_percent_floor as u64,
                        "source": "memory_pressure -Q",
                    })),
                    // A reading we cannot interpret is unknown, never "no pressure".
                    None => Observation::Unknown {
                        reason: "memory pressure reading had no free percentage".into(),
                    },
                }
            }
            unknown => unknown,
        }
    }
}

async fn probe_memory_pressure() -> Observation {
    let Some(output) = run_probe("memory_pressure", &["-Q"]).await else {
        return Observation::Unknown {
            reason: "memory_pressure command unavailable".into(),
        };
    };
    let prefix = "System-wide memory free percentage:";
    match output
        .lines()
        .find_map(|line| line.trim().strip_prefix(prefix))
        .and_then(|value| value.trim().trim_end_matches('%').parse::<u8>().ok())
    {
        Some(percent) => Observation::Observed(
            serde_json::json!({"system_free_percent":percent,"source":"memory_pressure -Q"}),
        ),
        None => Observation::Unknown {
            reason: "memory_pressure output did not contain a parseable free percentage".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poorai_domain::ModelChunk;
    use poorai_provider::ProviderError;

    fn thinking(text: &str) -> ModelChunk {
        ModelChunk {
            thinking: Some(text.into()),
            ..Default::default()
        }
    }
    fn call(name: &str) -> ModelChunk {
        ModelChunk {
            tool_calls: vec![ToolCall {
                name: name.into(),
                arguments: serde_json::json!({"value": "ok"}),
                id: None,
            }],
            ..Default::default()
        }
    }
    fn stream(
        chunks: Vec<Result<ModelChunk, ProviderError>>,
    ) -> Result<poorai_provider::ModelStream, ProviderError> {
        Ok(Box::pin(futures_util::stream::iter(chunks)))
    }

    /// The defect this probe had: a reasoning deployment opens with thinking
    /// chunks and emits its tool call near the end, so reading only the first
    /// chunk reported "no native tool call" for a model that made one.
    #[tokio::test]
    async fn tool_call_after_leading_thinking_chunks_is_observed() {
        let observed = drain_probe(
            stream(vec![
                Ok(thinking("The")),
                Ok(thinking(" user")),
                Ok(thinking(" wants")),
                Ok(call(PROBE_TOOL)),
                Ok(ModelChunk {
                    done: true,
                    ..Default::default()
                }),
            ]),
            Some(PROBE_TOOL),
        )
        .await
        .unwrap();
        assert_eq!(observed.matched_chunk_index, Some(4));
        assert_eq!(observed.matched_tool_call.unwrap().arguments["value"], "ok");
    }

    #[tokio::test]
    async fn prose_only_stream_does_not_claim_tool_support() {
        let observed = drain_probe(
            stream(vec![
                Ok(ModelChunk {
                    content: "I cannot".into(),
                    ..Default::default()
                }),
                Ok(ModelChunk {
                    content: " do that".into(),
                    done: true,
                    ..Default::default()
                }),
            ]),
            Some(PROBE_TOOL),
        )
        .await
        .unwrap();
        assert!(observed.matched_tool_call.is_none());
        assert!(observed.tool_call_names.is_empty());
        assert!(observed.produced_text);
    }

    /// A JSON array in prose is not a native call. The typed channel is the
    /// only evidence; content is never re-parsed to infer one.
    #[tokio::test]
    async fn json_shaped_prose_is_not_mistaken_for_a_tool_call() {
        let observed = drain_probe(
            stream(vec![Ok(ModelChunk {
                content: r#"[{"name":"probe_echo"}]"#.into(),
                done: true,
                ..Default::default()
            })]),
            Some(PROBE_TOOL),
        )
        .await
        .unwrap();
        assert!(observed.matched_tool_call.is_none());
    }

    #[tokio::test]
    async fn call_to_an_unoffered_tool_is_not_credited() {
        let observed = drain_probe(
            stream(vec![
                Ok(call("some_other_tool")),
                Ok(ModelChunk {
                    done: true,
                    ..Default::default()
                }),
            ]),
            Some(PROBE_TOOL),
        )
        .await
        .unwrap();
        assert!(observed.matched_tool_call.is_none());
        assert_eq!(observed.tool_call_names, vec!["some_other_tool"]);
    }

    /// Evidence already gathered survives a stream that breaks afterwards.
    #[tokio::test]
    async fn tool_call_survives_a_later_stream_error() {
        let observed = drain_probe(
            stream(vec![
                Ok(thinking("hm")),
                Ok(call(PROBE_TOOL)),
                Err(ProviderError::Protocol {
                    safe_context: "truncated".into(),
                }),
            ]),
            Some(PROBE_TOOL),
        )
        .await
        .unwrap();
        assert!(observed.matched_tool_call.is_some());
    }

    #[tokio::test]
    async fn immediate_failure_is_reported_rather_than_denied() {
        let error = drain_probe(
            stream(vec![Err(ProviderError::Timeout {
                safe_context: "local Ollama request".into(),
            })]),
            Some(PROBE_TOOL),
        )
        .await
        .unwrap_err();
        // A timeout must never read as "the model lacks tool support".
        assert!(error.contains("timed out"));
    }

    fn trial(n: u32) -> serde_json::Value {
        serde_json::json!({"trial": n, "arguments": {"value": "ok"}})
    }

    #[tokio::test]
    async fn abandons_a_stream_once_generation_is_underway() {
        // More chunks available than we intend to read: the drop is what ends it.
        let chunks: Vec<_> = (0..50)
            .map(|_| {
                Ok(ModelChunk {
                    content: "1\n".into(),
                    ..Default::default()
                })
            })
            .collect();
        let read = abandon_mid_stream(stream(chunks)).await.unwrap();
        assert_eq!(read, CANCEL_AFTER_CHUNKS);
    }

    /// A generation that finishes on its own was never interrupted, so it is no
    /// evidence that the deployment can be cancelled.
    #[tokio::test]
    async fn a_stream_that_completes_first_is_not_cancellation_evidence() {
        let reason = abandon_mid_stream(stream(vec![
            Ok(ModelChunk {
                content: "1".into(),
                ..Default::default()
            }),
            Ok(ModelChunk {
                content: "2".into(),
                done: true,
                ..Default::default()
            }),
        ]))
        .await
        .unwrap_err();
        assert!(reason.contains("before it could be interrupted"));
    }

    #[tokio::test]
    async fn a_stream_shorter_than_the_interrupt_point_is_unknown() {
        let reason = abandon_mid_stream(stream(vec![Ok(ModelChunk {
            content: "1".into(),
            ..Default::default()
        })]))
        .await
        .unwrap_err();
        assert!(reason.contains("ended after 1 chunk"));
    }

    #[tokio::test]
    async fn cancellation_probe_surfaces_a_failed_start() {
        let reason = abandon_mid_stream(Err(ProviderError::Unavailable {
            safe_context: "local Ollama request".into(),
        }))
        .await
        .unwrap_err();
        assert!(reason.contains("cancellation probe failed"));
    }

    #[test]
    fn unanimous_trials_are_reported_as_reliable() {
        let observed = summarize_tool_trials(3, vec![trial(1), trial(2), trial(3)], vec![]);
        let Observation::Observed(value) = observed else {
            panic!("expected an observation");
        };
        assert_eq!(value["calls"], 3);
        assert_eq!(value["reliable"], true);
    }

    /// Nemotron emits a native call only on some runs. The capability is real,
    /// but a caller must be able to see it is not dependable.
    #[test]
    fn intermittent_trials_are_observed_but_not_reliable() {
        let observed = summarize_tool_trials(
            3,
            vec![trial(2)],
            vec![
                "trial 1: no native tool call".into(),
                "trial 3: no native tool call".into(),
            ],
        );
        let Observation::Observed(value) = observed else {
            panic!("expected an observation");
        };
        assert_eq!(value["calls"], 1);
        assert_eq!(value["trials"], 3);
        assert_eq!(value["reliable"], false);
        // The failing trials stay in the record; a rate is not evidence unless
        // the misses are visible too.
        assert_eq!(value["failures"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn zero_calls_is_unknown_and_names_every_trial() {
        let observed = summarize_tool_trials(
            2,
            vec![],
            vec![
                "trial 1: timed out".into(),
                "trial 2: no native tool call".into(),
            ],
        );
        let Observation::Unknown { reason } = observed else {
            panic!("absence of a call is not proof of absent support");
        };
        assert!(reason.contains("trial 1: timed out"));
        assert!(reason.contains("trial 2"));
    }

    #[tokio::test]
    async fn empty_stream_is_unknown_not_absent() {
        assert!(drain_probe(stream(vec![]), Some(PROBE_TOOL)).await.is_err());
    }

    #[tokio::test]
    async fn single_chunk_reply_does_not_demonstrate_streaming() {
        let observed = drain_probe(
            stream(vec![Ok(ModelChunk {
                content: "OK".into(),
                done: true,
                ..Default::default()
            })]),
            None,
        )
        .await
        .unwrap();
        assert_eq!(observed.chunks, 1);
    }

    #[tokio::test]
    async fn drain_stops_at_the_chunk_bound() {
        let endless: Vec<_> = (0..PROBE_MAX_CHUNKS + 500)
            .map(|_| Ok(thinking("x")))
            .collect();
        let observed = drain_probe(stream(endless), Some(PROBE_TOOL))
            .await
            .unwrap();
        assert_eq!(observed.chunks, PROBE_MAX_CHUNKS);
    }
}
