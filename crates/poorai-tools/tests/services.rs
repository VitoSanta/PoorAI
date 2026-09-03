//! Standing a service up, and being sure it is gone.
//!
//! `LocalService` opened a port and nothing used it. `run_command` cannot start
//! a server: it waits for the process to exit, which is the one thing a server
//! does not do.

use poorai_tools::service::ServiceSupervisor;
use poorai_tools::{Approval, SandboxPolicy, ToolPolicy};
use std::path::Path;
use std::time::Duration;

fn policy(root: &Path, approvals: Vec<Approval>) -> ToolPolicy {
    ToolPolicy {
        root: root.to_path_buf(),
        extra_readable: Vec::new(),
        allow_commands: vec!["python3".into(), "sh".into(), "sleep".into()],
        output_limit: 16 * 1024,
        timeout: Duration::from_secs(20),
        // The supervisor is what is under test here; the sandbox has its own
        // fixtures and adding it would make a failure ambiguous.
        sandbox: SandboxPolicy::Disabled,
        approvals,
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Runtime::new().unwrap().block_on(future)
}

fn listening(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(200),
    )
    .is_ok()
}

/// A process that outlives the action that started it needs the grant that
/// says local services are allowed at all.
#[test]
fn starting_a_service_needs_the_local_service_grant() {
    let root = tempfile::tempdir().unwrap();
    let mut supervisor = ServiceSupervisor::new();
    let refused = block_on(supervisor.start(
        &policy(root.path(), vec![]),
        "sleep",
        &["30".to_string()],
        None,
    ));
    assert!(refused.is_err(), "a service started with no grant");
    assert!(supervisor.running().is_empty());
}

/// A service is ready when it accepts a connection, not when its process
/// exists: a server that has been spawned and has not yet bound refuses every
/// request, and a test that races it fails for reasons unrelated to the code.
#[test]
fn a_service_is_started_waited_for_and_stopped() {
    let root = tempfile::tempdir().unwrap();
    let port = ServiceSupervisor::reserve_port().unwrap();
    let mut supervisor = ServiceSupervisor::new();
    let handle = block_on(supervisor.start(
        &policy(root.path(), vec![Approval::LocalService]),
        "python3",
        &[
            "-c".to_string(),
            format!(
                "import http.server as h; h.HTTPServer(('127.0.0.1',{port}), h.SimpleHTTPRequestHandler).serve_forever()"
            ),
        ],
        Some(port),
    ))
    .unwrap();

    block_on(supervisor.wait_until_ready(handle.id, port, Duration::from_secs(15)))
        .expect("the service never accepted a connection");
    assert!(listening(port));
    assert_eq!(supervisor.running().len(), 1);

    let outcome = block_on(supervisor.stop(handle.id, 8192)).unwrap();
    assert_eq!(outcome.id, handle.id);
    assert!(supervisor.running().is_empty());
    assert!(!listening(port), "the port is still held after stop");
}

/// The failure this exists to prevent: a run that crashes, is killed, or simply
/// forgets, leaving a process holding a port on someone's machine.
#[test]
fn dropping_the_supervisor_kills_what_it_started() {
    let root = tempfile::tempdir().unwrap();
    let port = ServiceSupervisor::reserve_port().unwrap();
    {
        let mut supervisor = ServiceSupervisor::new();
        let handle = block_on(supervisor.start(
            &policy(root.path(), vec![Approval::LocalService]),
            "python3",
            &[
                "-c".to_string(),
                format!(
                    "import http.server as h; h.HTTPServer(('127.0.0.1',{port}), h.SimpleHTTPRequestHandler).serve_forever()"
                ),
            ],
            Some(port),
        ))
        .unwrap();
        block_on(supervisor.wait_until_ready(handle.id, port, Duration::from_secs(15))).unwrap();
        assert!(listening(port));
        // and the run ends here, without stopping anything.
    }
    // The kill is a signal, so give the OS a moment to act on it.
    for _ in 0..40 {
        if !listening(port) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!listening(port), "a service outlived its supervisor");
}

/// Waiting the full timeout to discover a process died at once wastes the time
/// the caller gave for starting rather than for failing.
#[test]
fn a_service_that_exits_immediately_is_not_waited_out() {
    let root = tempfile::tempdir().unwrap();
    let port = ServiceSupervisor::reserve_port().unwrap();
    let mut supervisor = ServiceSupervisor::new();
    let handle = block_on(supervisor.start(
        &policy(root.path(), vec![Approval::LocalService]),
        "sh",
        &["-c".to_string(), "exit 1".to_string()],
        Some(port),
    ))
    .unwrap();
    let started = std::time::Instant::now();
    let waited = block_on(supervisor.wait_until_ready(handle.id, port, Duration::from_secs(20)));
    assert!(waited.is_err());
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the full timeout was spent on a process that had already exited"
    );
}

#[test]
fn a_reserved_port_is_free_when_it_is_handed_over() {
    let first = ServiceSupervisor::reserve_port().unwrap();
    let second = ServiceSupervisor::reserve_port().unwrap();
    assert_ne!(first, second, "the same port was reserved twice");
    // Closed before it is handed over, which is the race the doc comment
    // names: it must be bindable now.
    assert!(std::net::TcpListener::bind(("127.0.0.1", first)).is_ok());
}

#[test]
fn stopping_a_service_that_is_not_running_says_so() {
    let mut supervisor = ServiceSupervisor::new();
    assert!(block_on(supervisor.stop(99, 1024)).is_err());
}
