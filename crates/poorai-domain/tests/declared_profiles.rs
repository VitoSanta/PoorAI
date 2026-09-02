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

/// A context default measured on this machine is a different claim from one
/// copied out of a specification, and the two must not read alike.
#[test]
fn a_measured_context_default_says_so() {
    let profiles = declared();
    let measured: Vec<&str> = profiles
        .iter()
        .filter(|p| p.context_source == ParameterSource::HardwareCalibration)
        .map(|p| p.model_selector.as_str())
        .collect();
    // The four deployments a ladder was actually run against.
    assert_eq!(measured.len(), 4);
    for selector in ["ornith-1.5:35b", "qwen3.8:27b-mlx", "gpt-oss:20b"] {
        assert!(measured.contains(&selector), "{selector} was measured");
    }
    // A measured profile carries the numbers, not just the claim.
    let ornith = ModelProfile::select(&profiles, "ornith-1.5:35b").unwrap();
    assert!(ornith.provenance.contains("tok/s"));
    assert_eq!(ornith.context.default, ornith.context.maximum);
}

/// The one context choice with a measured price says what the price is.
#[test]
fn the_contested_default_records_its_cost() {
    let profiles = declared();
    let qwen = ModelProfile::select(&profiles, "qwen3.8:27b-mlx").unwrap();
    assert!(qwen.provenance.contains("monotonically"));
    assert!(qwen.provenance.contains("least reliable"));
    // Not taken to the ceiling, unlike the deployment where it costs nothing.
    assert!(qwen.context.default < qwen.context.maximum);
}
