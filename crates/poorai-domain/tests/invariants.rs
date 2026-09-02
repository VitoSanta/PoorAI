//! Property tests for the invariants every persisted artifact depends on.
//!
//! These exercise the crate through its public contract, the way the store,
//! adapters and CLI see it.

use poorai_domain::*;
use proptest::prelude::*;
use std::collections::BTreeMap;

fn any_provenance() -> impl Strategy<Value = Provenance> {
    ("[a-z:/]{1,20}", "[a-f0-9]{8}").prop_map(|(source, hash)| Provenance {
        source,
        observed_at: now(),
        content_hash: hash,
    })
}

fn any_observation() -> impl Strategy<Value = Observation> {
    prop_oneof![
        any::<bool>().prop_map(|b| Observation::Observed(serde_json::json!(b))),
        "[a-z ]{0,40}".prop_map(|reason| Observation::Unknown { reason }),
    ]
}

fn any_deployment() -> impl Strategy<Value = DeploymentDescriptor> {
    (
        "[a-z]{1,10}",
        "https?://[a-z]{1,10}/",
        "[a-z0-9.:_-]{1,20}",
        proptest::collection::btree_map("[a-z]{1,5}", "[a-z]{1,5}", 0..3),
        proptest::option::of("[a-z]{1,8}"),
    )
        .prop_map(
            |(provider, endpoint, model_ref, backend_options, auth_ref)| DeploymentDescriptor {
                schema_version: SCHEMA_VERSION,
                id: new_id(),
                provider,
                endpoint,
                model_ref,
                backend_options,
                auth_ref,
            },
        )
}

/// Points that meet the default thresholds, so a profile built from them is
/// valid. Points that fail thresholds are exercised by their own properties.
fn any_stable_point() -> impl Strategy<Value = StablePoint> {
    (1u32..131_072, 3u32..10, 0.0f64..=1.0).prop_map(|(context_tokens, samples, _)| StablePoint {
        context_tokens,
        samples,
        success_rate: 1.0,
        median_first_token_ms: 1.0,
        generation_tokens_per_second: 1.0,
        variance: 0.0,
        memory_pressure_observed: false,
    })
}

fn calibration_with(points: Vec<StablePoint>, key: &str) -> CalibrationProfile {
    CalibrationProfile {
        schema_version: SCHEMA_VERSION,
        id: new_id(),
        compatibility_key: key.into(),
        model_digest: "digest".into(),
        deployment_fingerprint: "fingerprint".into(),
        harness_rev: "harness".into(),
        thresholds: CalibrationThresholds::default(),
        stable_points: points,
        raw_artifact_hashes: vec![],
        created_at: now(),
    }
}

fn execution_for(
    calibration: Option<&CalibrationProfile>,
    context_tokens: u32,
    reserve_tokens: u32,
    evidence: EvidenceLabel,
) -> ExecutionProfile {
    ExecutionProfile {
        schema_version: SCHEMA_VERSION,
        id: new_id(),
        strategy_id: new_id(),
        calibration_id: calibration.map(|c| c.id),
        context_tokens,
        reserve_tokens,
        concurrency: 1,
        budgets: serde_json::json!({}),
        rationale: "property test".into(),
        evidence,
        compatibility_key: calibration
            .map(|c| c.compatibility_key.clone())
            .unwrap_or_default(),
    }
}

proptest! {
    /// An `Observation` must never round-trip into the other variant. A collapse
    /// here would silently turn "we did not observe this" into "observed".
    #[test]
    fn observation_round_trips_without_changing_variant(observation in any_observation()) {
        let encoded = serde_json::to_string(&observation).unwrap();
        let decoded: Observation = serde_json::from_str(&encoded).unwrap();
        prop_assert_eq!(&observation, &decoded);
        match observation {
            Observation::Observed(_) => prop_assert!(encoded.contains("\"observed\"")),
            Observation::Unknown { .. } => prop_assert!(encoded.contains("\"unknown\"")),
        }
    }

    /// `skip_serializing_if` must not drop a payload that was present.
    #[test]
    fn model_chunk_round_trips_every_channel(
        content in "[a-z ]{0,30}",
        thinking in proptest::option::of("[a-z ]{1,30}"),
        calls in proptest::collection::vec("[a-z_]{1,12}", 0..4),
        generated_tokens in proptest::option::of(any::<u64>()),
        done in any::<bool>(),
    ) {
        let chunk = ModelChunk {
            content,
            thinking,
            metrics: generated_tokens.map(|generated_tokens| GenerationMetrics {
                generated_tokens: Some(generated_tokens),
                generation_duration_ns: Some(1_000_000_000),
                ..Default::default()
            }),
            tool_calls: calls
                .into_iter()
                .map(|name| ToolCall {
                    name,
                    arguments: serde_json::json!({"value": "ok"}),
                    id: None,
                })
                .collect(),
            done,
        };
        let decoded: ModelChunk =
            serde_json::from_str(&serde_json::to_string(&chunk).unwrap()).unwrap();
        prop_assert_eq!(&chunk, &decoded);
        prop_assert_eq!(chunk.tool_calls.len(), decoded.tool_calls.len());
        // Backend-reported counts must survive the round trip, or a calibration
        // artifact loses the numbers its rate was computed from.
        prop_assert_eq!(
            chunk.metrics.as_ref().and_then(|m| m.generated_tokens),
            decoded.metrics.as_ref().and_then(|m| m.generated_tokens)
        );
    }

    /// A rate is reported only when the backend gave enough to compute one.
    #[test]
    fn a_reported_rate_requires_both_a_count_and_a_duration(
        tokens in proptest::option::of(1u64..10_000),
        nanos in proptest::option::of(0u64..10_000_000_000),
    ) {
        let metrics = GenerationMetrics {
            generated_tokens: tokens,
            generation_duration_ns: nanos,
            ..Default::default()
        };
        let computable = tokens.is_some() && nanos.is_some_and(|n| n > 0);
        prop_assert_eq!(metrics.tokens_per_second().is_some(), computable);
    }

    #[test]
    fn model_definition_round_trips(
        digest in "[a-f0-9:]{4,20}",
        capabilities in proptest::collection::btree_map("[a-z_]{1,12}", any_observation(), 0..5),
        provenance in any_provenance(),
    ) {
        let definition = ModelDefinition {
            schema_version: SCHEMA_VERSION,
            id: new_id(),
            digest,
            family: None,
            quantization: None,
            capabilities,
            metadata: serde_json::json!({"model_info": {}}),
            provenance,
        };
        let decoded: ModelDefinition =
            serde_json::from_str(&serde_json::to_string(&definition).unwrap()).unwrap();
        prop_assert_eq!(definition, decoded);
    }

    /// The fingerprint is an invalidation key: it must depend on every field
    /// that changes what is being served, and on nothing else.
    #[test]
    fn fingerprint_ignores_identity_and_credentials(deployment in any_deployment()) {
        let mut other = deployment.clone();
        other.id = new_id();
        other.auth_ref = Some("rotated-credential-ref".into());
        prop_assert_eq!(deployment.fingerprint(), other.fingerprint());
    }

    #[test]
    fn fingerprint_changes_with_the_served_model(deployment in any_deployment()) {
        let mut other = deployment.clone();
        other.model_ref = format!("{}-different", deployment.model_ref);
        prop_assert_ne!(deployment.fingerprint(), other.fingerprint());
    }

    #[test]
    fn fingerprint_changes_with_backend_options(deployment in any_deployment()) {
        let mut other = deployment.clone();
        other
            .backend_options
            .insert("num_ctx".into(), "8192".into());
        prop_assert_ne!(deployment.fingerprint(), other.fingerprint());
    }

    #[test]
    fn hashing_is_deterministic_and_collision_sensitive(
        left in ".{0,64}",
        right in ".{0,64}",
    ) {
        prop_assert_eq!(hash_bytes(&left), hash_bytes(&left));
        prop_assert_eq!(left == right, hash_bytes(&left) == hash_bytes(&right));
    }

    #[test]
    fn identifiers_are_time_ordered_and_unique(count in 2usize..32) {
        let ids: Vec<Id> = (0..count).map(|_| new_id()).collect();
        for id in &ids {
            prop_assert_eq!(id.get_version_num(), 7);
        }
        let mut sorted = ids.clone();
        sorted.sort();
        // UUIDv7 is time-ordered, so generation order must be sort order.
        prop_assert_eq!(&ids, &sorted);
        sorted.dedup();
        prop_assert_eq!(sorted.len(), count);
    }

    /// Calibration requires repeated measurement. Fewer than three samples is
    /// not a stable point regardless of how good the numbers look.
    #[test]
    fn calibration_rejects_under_sampled_points(
        context_tokens in 1u32..131_072,
        samples in 0u32..3,
    ) {
        let profile = calibration_with(
            vec![StablePoint {
                context_tokens,
                samples,
                success_rate: 1.0,
                median_first_token_ms: 1.0,
                generation_tokens_per_second: 1.0,
                variance: 0.0,
                memory_pressure_observed: false,
            }],
            "key",
        );
        prop_assert!(profile.validate().is_err());
    }

    #[test]
    fn calibration_rejects_success_rates_outside_the_unit_interval(
        success_rate in prop_oneof![-100.0f64..-0.001, 1.001f64..100.0],
    ) {
        let profile = calibration_with(
            vec![StablePoint {
                context_tokens: 4096,
                samples: 3,
                success_rate,
                median_first_token_ms: 1.0,
                generation_tokens_per_second: 1.0,
                variance: 0.0,
                memory_pressure_observed: false,
            }],
            "key",
        );
        prop_assert!(profile.validate().is_err());
    }

    #[test]
    fn calibration_accepts_measured_points(points in proptest::collection::vec(any_stable_point(), 1..5)) {
        prop_assert!(calibration_with(points, "key").validate().is_ok());
    }

    /// Capacity must come from a measurement that succeeded, not merely one
    /// attempted at that size. A tier where every sample failed is a record of
    /// failure; authorising execution from it is inventing capacity.
    #[test]
    fn a_tier_that_failed_its_thresholds_authorises_nothing(
        context_tokens in 1u32..131_072,
        success_rate in 0.0f64..1.0,
    ) {
        let failed = StablePoint {
            context_tokens,
            samples: 3,
            success_rate,
            median_first_token_ms: 1.0,
            generation_tokens_per_second: 0.0,
            variance: 0.0,
            memory_pressure_observed: false,
        };
        let calibration = calibration_with(vec![failed], "key");
        // The profile itself must refuse to hold a point below its thresholds.
        prop_assert!(calibration.validate().is_err());
        let profile = execution_for(
            Some(&calibration),
            context_tokens,
            0,
            EvidenceLabel::Measured,
        );
        prop_assert!(profile.validate_against(Some(&calibration)).is_err());
    }

    /// Memory pressure during measurement disqualifies the point unless the
    /// profile declares that pressure is acceptable.
    #[test]
    fn a_tier_measured_under_memory_pressure_authorises_nothing(
        context_tokens in 1u32..131_072,
    ) {
        let pressured = StablePoint {
            context_tokens,
            samples: 3,
            success_rate: 1.0,
            median_first_token_ms: 1.0,
            generation_tokens_per_second: 1.0,
            variance: 0.0,
            memory_pressure_observed: true,
        };
        let calibration = calibration_with(vec![pressured], "key");
        prop_assert!(calibration.validate().is_err());
        let profile = execution_for(
            Some(&calibration),
            context_tokens,
            0,
            EvidenceLabel::Measured,
        );
        prop_assert!(profile.validate_against(Some(&calibration)).is_err());
    }

    /// MASTER_SPEC rule 4: capacity comes from evidence, never extrapolation.
    /// A measured profile is accepted only when a measured point covers the
    /// requested context.
    #[test]
    fn measured_context_never_exceeds_a_measured_stable_point(
        points in proptest::collection::vec(any_stable_point(), 1..5),
        context_tokens in 1u32..131_072,
    ) {
        let calibration = calibration_with(points, "key");
        let profile = execution_for(
            Some(&calibration),
            context_tokens,
            0,
            EvidenceLabel::Measured,
        );
        let covered = calibration
            .stable_points
            .iter()
            .any(|p| p.context_tokens >= context_tokens);
        prop_assert_eq!(profile.validate_against(Some(&calibration)).is_ok(), covered);
    }

    /// A bootstrap profile is the uncalibrated fallback. It must never be able
    /// to borrow authority from a calibration it is not bound to.
    #[test]
    fn bootstrap_evidence_is_rejected_when_calibration_is_supplied(
        points in proptest::collection::vec(any_stable_point(), 1..5),
    ) {
        let calibration = calibration_with(points, "key");
        let profile = execution_for(
            Some(&calibration),
            1024,
            0,
            EvidenceLabel::ConservativeBootstrap,
        );
        prop_assert!(profile.validate_against(Some(&calibration)).is_err());
    }

    /// Claiming measured evidence without a calibration to back it is always
    /// invalid, whatever the numbers say.
    #[test]
    fn measured_evidence_requires_a_calibration(context_tokens in 1u32..131_072) {
        let profile = execution_for(None, context_tokens, 0, EvidenceLabel::Measured);
        prop_assert!(profile.validate_against(None).is_err());
    }

    /// The safety reserve must leave room for output. Reserve at or above the
    /// budget leaves none.
    #[test]
    fn reserve_never_consumes_the_whole_context(
        context_tokens in 1u32..65_536,
        excess in 0u32..1024,
    ) {
        let points = vec![StablePoint {
            context_tokens,
            samples: 3,
            success_rate: 1.0,
            median_first_token_ms: 1.0,
            generation_tokens_per_second: 1.0,
            variance: 0.0,
            memory_pressure_observed: false,
        }];
        let calibration = calibration_with(points, "key");
        let profile = execution_for(
            Some(&calibration),
            context_tokens,
            context_tokens + excess,
            EvidenceLabel::Measured,
        );
        prop_assert!(profile.validate_against(Some(&calibration)).is_err());
    }

    /// A calibration measured on different hardware or a different backend must
    /// not authorise this profile.
    #[test]
    fn incompatible_calibration_is_rejected(
        points in proptest::collection::vec(any_stable_point(), 1..5),
        key in "[a-z]{1,8}",
    ) {
        let calibration = calibration_with(points, &key);
        let mut profile = execution_for(Some(&calibration), 1, 0, EvidenceLabel::Measured);
        profile.compatibility_key = format!("{key}-other-machine");
        prop_assert!(profile.validate_against(Some(&calibration)).is_err());
    }

    #[test]
    fn deployment_validation_requires_an_http_endpoint(
        endpoint in "[a-z][a-z0-9+.-]{0,8}://[a-z]{1,8}/",
    ) {
        let deployment = DeploymentDescriptor {
            schema_version: SCHEMA_VERSION,
            id: new_id(),
            provider: "ollama".into(),
            endpoint: endpoint.clone(),
            model_ref: "model".into(),
            backend_options: BTreeMap::new(),
            auth_ref: None,
        };
        let http = endpoint.starts_with("http://") || endpoint.starts_with("https://");
        prop_assert_eq!(deployment.validate().is_ok(), http);
    }

    /// Provenance is what makes an artifact auditable; an evaluation without it
    /// is not a result.
    #[test]
    fn evaluation_requires_full_provenance(
        corpus_rev in "[a-z0-9]{0,6}",
        harness_rev in "[a-z0-9]{0,6}",
        model_digest in "[a-z0-9]{0,6}",
        outcome_hash in "[a-z0-9]{0,6}",
    ) {
        let run = EvaluationRun {
            schema_version: SCHEMA_VERSION,
            id: new_id(),
            corpus_rev: corpus_rev.clone(),
            task_set: "suite".into(),
            execution_profile_id: new_id(),
            model_digest: model_digest.clone(),
            deployment_fingerprint: "fingerprint".into(),
            hardware_compatibility_key: "key".into(),
            harness_rev: harness_rev.clone(),
            seeds: vec![1],
            outcome_hash: outcome_hash.clone(),
            artifact_hashes: vec![],
            created_at: now(),
        };
        let complete = ![&corpus_rev, &harness_rev, &model_digest, &outcome_hash]
            .iter()
            .any(|field| field.is_empty());
        prop_assert_eq!(run.validate().is_ok(), complete);
    }
}

// -------------------------------------------------------------- strategies

#[test]
fn a_strategy_applies_only_to_the_deployment_it_names() {
    let strategy = |selector: &str| ModelStrategy {
        schema_version: SCHEMA_VERSION,
        id: new_id(),
        model_selector: selector.into(),
        role: "control".into(),
        prompt_suffix: " extra".into(),
        max_actions: Some(12),
        retrieval_excerpts: Some(8),
        plan_first: false,
        rationale: "measured".into(),
    };
    let declared = vec![strategy("muse-glimmer:30b-mlx"), strategy("ornith-1.5:35b")];
    assert_eq!(
        ModelStrategy::select(&declared, "ornith-1.5:35b").map(|s| s.model_selector.as_str()),
        Some("ornith-1.5:35b")
    );
    // Selection is exact: a near miss gets the shared default, not someone
    // else's policy.
    assert!(ModelStrategy::select(&declared, "ornith-1.5:35b-mlx").is_none());
    assert!(ModelStrategy::select(&declared, "qwen3.8:27b-mlx").is_none());
    assert!(ModelStrategy::select(&[], "ornith-1.5:35b").is_none());
}

#[test]
fn a_strategy_round_trips_and_keeps_its_rationale() {
    let strategy = ModelStrategy {
        schema_version: SCHEMA_VERSION,
        id: new_id(),
        model_selector: "m".into(),
        role: "r".into(),
        prompt_suffix: " suffix".into(),
        max_actions: None,
        retrieval_excerpts: None,
        plan_first: false,
        rationale: "why this exists".into(),
    };
    let decoded: ModelStrategy =
        serde_json::from_str(&serde_json::to_string(&strategy).unwrap()).unwrap();
    assert_eq!(strategy, decoded);
    // A strategy without its reason is an opinion with a schema.
    assert!(!decoded.rationale.is_empty());
}

// ---------------------------------------------------------- model profiles

fn profile(selector: &str, maximum: u32) -> ModelProfile {
    ModelProfile {
        schema_version: SCHEMA_VERSION,
        model_selector: selector.into(),
        context: ContextPolicy {
            minimum: 65_536,
            default: 131_072,
            maximum,
        },
        sampling: BTreeMap::from([(
            "temperature".to_string(),
            ResolvedParameter {
                value: serde_json::json!(0.6),
                source: ParameterSource::OfficialModelCard,
            },
        )]),
        reasoning: None,
        provenance: "vendor card".into(),
    }
}

/// A request for more context than the tag declares would either be refused or
/// silently ignored, and both make the recorded number a fiction.
#[test]
fn context_is_clamped_to_what_the_tag_declares() {
    let p = profile("m", 131_072);
    assert_eq!(p.context_for(Some(1_000_000)), 131_072);
    assert_eq!(p.context_for(Some(1024)), 65_536);
    assert_eq!(p.context_for(None), 131_072);
    // A tag with a larger ceiling allows more.
    assert_eq!(profile("m", 262_144).context_for(Some(262_144)), 262_144);
}

/// A value without its origin cannot be compared with another run's: a
/// temperature the vendor recommends and one nobody chose look identical.
#[test]
fn every_sampling_value_carries_where_it_came_from() {
    let p = profile("m", 131_072);
    assert_eq!(
        p.sampling["temperature"].source,
        ParameterSource::OfficialModelCard
    );
    // What the backend receives is the value alone.
    assert_eq!(p.sampling_options()["temperature"], serde_json::json!(0.6));
}

#[test]
fn a_profile_applies_only_to_the_tag_it_names() {
    let declared = vec![profile("ornith-1.5:35b", 262_144)];
    assert!(ModelProfile::select(&declared, "ornith-1.5:35b").is_some());
    // Per tag, not per family: the same model under another tag can declare a
    // different limit.
    assert!(ModelProfile::select(&declared, "ornith-1.5:35b-mlx").is_none());
}
