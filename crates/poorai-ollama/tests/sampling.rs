//! Sampling controls must reach the backend.
//!
//! A run that records a seed it never sent is not reproducible, whatever the
//! record says.

use poorai_domain::{ChatMessage, DeploymentDescriptor, ModelRequest};
use poorai_ollama::OllamaProvider;
use poorai_provider::ModelProvider;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

/// Captures the request body the provider actually sent.
fn capturing_server() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 8192];
        let read = stream.read(&mut buffer).unwrap();
        let text = String::from_utf8_lossy(&buffer[..read]).to_string();
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        let _ = tx.send(body);
        let payload = r#"{"message":{"role":"assistant","content":"ok"},"done":true}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            payload.len(),
            payload
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    (format!("http://{address}/"), rx)
}

fn request(endpoint: &str, seed: Option<u64>, temperature_milli: Option<u64>) -> ModelRequest {
    ModelRequest {
        deployment: DeploymentDescriptor {
            schema_version: 1,
            id: poorai_domain::new_id(),
            provider: "ollama".into(),
            endpoint: endpoint.into(),
            model_ref: "fixture".into(),
            backend_options: Default::default(),
            auth_ref: None,
        },
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "hi".into(),
        }],
        context_tokens: 4096,
        tools: None,
        seed,
        temperature_milli,
    }
}

#[tokio::test]
async fn a_seed_and_temperature_reach_the_backend() {
    let (endpoint, rx) = capturing_server();
    let provider = OllamaProvider::new(&endpoint, Duration::from_secs(5)).unwrap();
    let _ = provider.chat(request(&endpoint, Some(4242), Some(0))).await;
    let body: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
    assert_eq!(body["options"]["seed"], 4242);
    assert_eq!(body["options"]["temperature"], 0.0);
    assert_eq!(body["options"]["num_ctx"], 4096);
}

#[tokio::test]
async fn an_unset_control_is_left_to_the_backend_default() {
    let (endpoint, rx) = capturing_server();
    let provider = OllamaProvider::new(&endpoint, Duration::from_secs(5)).unwrap();
    let _ = provider.chat(request(&endpoint, None, None)).await;
    let body: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
    // Absent rather than guessed at: sending a default we invented would be a
    // configuration the caller never chose.
    assert!(body["options"].get("seed").is_none());
    assert!(body["options"].get("temperature").is_none());
}

#[tokio::test]
async fn a_fractional_temperature_is_sent_as_a_fraction() {
    let (endpoint, rx) = capturing_server();
    let provider = OllamaProvider::new(&endpoint, Duration::from_secs(5)).unwrap();
    let _ = provider.chat(request(&endpoint, None, Some(700))).await;
    let body: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
    assert_eq!(body["options"]["temperature"], 0.7);
}
