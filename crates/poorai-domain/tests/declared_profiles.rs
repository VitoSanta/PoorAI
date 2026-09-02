//! The declared profiles, checked as data rather than trusted as configuration.

use poorai_domain::{ModelProfile, ParameterSource};

fn declared() -> Vec<ModelProfile> {
    #[derive(serde::Deserialize)]
    struct File {
        profiles: Vec<ModelProfile>,
    }
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../strategies/models.json"
    ))
    .expect("strategies/models.json");
    serde_json::from_slice::<File>(&bytes).unwrap().profiles
}

#[test]
fn every_profile_is_coherent_and_explains_itself() {
    let profiles = declared();
    assert_eq!(profiles.len(), 7);
    for profile in &profiles {
        assert!(
            profile.context.is_coherent(),
            "{} has contradictory context sizes",
            profile.model_selector
        );
        // A profile without a reason is configuration nobody can argue with.
        assert!(
            profile.provenance.len() > 60,
            "{} has no usable provenance",
            profile.model_selector
        );
        assert!(!profile.sampling.is_empty(), "{}", profile.model_selector);
    }
}

/// The product targets large repositories, so a deployment that cannot serve
/// the required context does not qualify however it scores. That is a
/// selection criterion, not a tuning knob, and it is visible in the data.
#[test]
fn every_qualifying_deployment_is_allocated_the_full_ceiling() {
    const REQUIRED: u32 = 262_144;
    let profiles = declared();
    for profile in &profiles {
        if profile.context.maximum >= REQUIRED {
            assert_eq!(
                profile.context.default, profile.context.maximum,
                "{} qualifies but is allocated less than it can serve",
                profile.model_selector
            );
            assert_eq!(profile.context.minimum, REQUIRED);
            assert_eq!(profile.context_source, ParameterSource::PoorAiOverride);
        } else {
            // Below the requirement, and the profile does not pretend
            // otherwise by raising a ceiling the tag cannot serve.
            assert!(profile.context.default <= profile.context.maximum);
        }
    }
    let qualifying = profiles
        .iter()
        .filter(|p| p.context.maximum >= REQUIRED)
        .count();
    assert_eq!(qualifying, 4);
}

/// The throughput cost of the choice stays recorded even though the choice was
/// made against it: a decision that hides its own price cannot be revisited.
#[test]
fn the_deployment_that_pays_for_context_still_records_the_price() {
    let profiles = declared();
    let qwen = ModelProfile::select(&profiles, "qwen3.8:27b-mlx").unwrap();
    assert_eq!(qwen.context.default, 262_144);
    assert!(qwen.provenance.contains("monotonically"));
    assert!(qwen.provenance.contains("least reliable"));
    assert!(qwen.provenance.contains("product requirement"));
}
