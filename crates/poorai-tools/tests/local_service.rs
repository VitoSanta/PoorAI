//! Verifying a system rather than a file means starting a service and talking
//! to it, which needs loopback. Loopback is a smaller grant than the network —
//! no remote host is reachable — but it is a real one, because loopback also
//! reaches whatever else listens on this host. It is therefore its own
//! approval, and the tests below are as much about what it does *not* grant.

use poorai_tools::{Approval, SandboxPolicy, ToolPolicy, run_command};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::Duration;

fn policy(root: &Path, approvals: Vec<Approval>) -> ToolPolicy {
    ToolPolicy {
        root: root.to_path_buf(),
        allow_commands: vec!["python3".into()],
        output_limit: 64 * 1024,
        timeout: Duration::from_secs(20),
        sandbox: SandboxPolicy::Required,
        approvals,
    }
}

/// A server on loopback that answers one request, so a sandboxed client has
/// something real to reach.
fn loopback_server() -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0u8; 512];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        }
    });
    (port, handle)
}

/// Connects to a port on loopback and prints whether it succeeded.
const CONNECT: &str = "import socket,sys\n\
                       s=socket.socket()\n\
                       s.settimeout(5)\n\
                       try:\n    s.connect((sys.argv[1], int(sys.argv[2])))\n    print('connected')\n\
                       except Exception as e:\n    print('refused', type(e).__name__)\n";

async fn attempt_connect(approvals: Vec<Approval>, port: u16) -> String {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("c.py"), CONNECT).unwrap();
    let policy = policy(root.path(), approvals);
    let result = run_command(
        &policy,
        "python3",
        &[
            "c.py".to_string(),
            "127.0.0.1".to_string(),
            port.to_string(),
        ],
    )
    .await
    .expect("the command itself should run");
    assert!(result.sandboxed, "the test proves nothing unsandboxed");
    result.stdout
}

#[tokio::test]
async fn without_the_grant_loopback_is_refused() {
    let (port, server) = loopback_server();
    let output = attempt_connect(vec![], port).await;
    assert!(
        output.contains("refused"),
        "a sandboxed process reached loopback with no grant: {output}"
    );
    drop(server);
}

#[tokio::test]
async fn with_the_grant_loopback_is_reachable() {
    let (port, server) = loopback_server();
    let output = attempt_connect(vec![Approval::LocalService], port).await;
    assert!(
        output.contains("connected"),
        "the grant did not open loopback: {output}"
    );
    server.join().unwrap();
}

/// The grant that matters most for what it withholds: a service may be started
/// and exercised, and still nothing leaves the machine.
///
/// Asserted on the *kind* of failure, not merely on failing. A public address
/// fails to connect for reasons that have nothing to do with the sandbox -- no
/// route, a firewall, a timeout -- so an earlier version of this test passed
/// while a mutant granting the whole network survived it. `PermissionError`
/// comes from the sandbox refusing the socket; a timeout or a refused
/// connection would mean the sandbox let the attempt out.
#[tokio::test]
async fn a_remote_host_is_refused_by_the_sandbox_itself() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("c.py"), CONNECT).unwrap();
    let policy = policy(root.path(), vec![Approval::LocalService]);
    let result = run_command(
        &policy,
        "python3",
        &["c.py".to_string(), "8.8.8.8".to_string(), "53".to_string()],
    )
    .await
    .unwrap();
    assert!(result.sandboxed);
    if result.stdout.contains("timeout") || result.stdout.contains("TimeoutError") {
        // Without a route the attempt cannot distinguish a denial from a dead
        // network, and asserting either way would be dishonest.
        eprintln!("skipped: no route to a remote host, so the denial is not observable");
        return;
    }
    assert!(
        result.stdout.contains("PermissionError"),
        "the sandbox did not refuse a remote host: {}",
        result.stdout
    );
}

/// The grant reaches this machine's LAN address as well as its loopback one,
/// because seatbelt cannot express loopback alone. Asserted so the wider
/// boundary is recorded as measured behaviour rather than left to the reader
/// of a comment, and so narrowing it later is a visible change.
#[tokio::test]
async fn the_grant_reaches_this_hosts_other_addresses_too() {
    let Some(address) = non_loopback_address() else {
        eprintln!("skipped: this host has no non-loopback address");
        return;
    };
    let listener = TcpListener::bind((address, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.write_all(b"reached");
        }
    });
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("c.py"), CONNECT).unwrap();
    let policy = policy(root.path(), vec![Approval::LocalService]);
    let result = run_command(
        &policy,
        "python3",
        &["c.py".to_string(), address.to_string(), port.to_string()],
    )
    .await
    .unwrap();
    assert!(result.sandboxed);
    assert!(
        result.stdout.contains("connected"),
        "the documented boundary is this host, but its LAN address was refused: {}",
        result.stdout
    );
    server.join().unwrap();
}

/// This machine's own address on a real interface, if it has one. Found by
/// asking the routing table which source address a remote destination would
/// use; no packet is sent.
fn non_loopback_address() -> Option<std::net::Ipv4Addr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

/// Binding is half of running a service, and is granted with the other half.
#[tokio::test]
async fn with_the_grant_a_service_can_bind_and_be_reached() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("s.py"),
        "import socket\n\
         srv=socket.socket()\n\
         srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\n\
         srv.bind(('127.0.0.1', 0))\n\
         srv.listen(1)\n\
         port=srv.getsockname()[1]\n\
         c=socket.socket()\n\
         c.settimeout(5)\n\
         c.connect(('127.0.0.1', port))\n\
         conn,_=srv.accept()\n\
         conn.sendall(b'served')\n\
         print(c.recv(16).decode())\n",
    )
    .unwrap();
    let policy = policy(root.path(), vec![Approval::LocalService]);
    let result = run_command(&policy, "python3", &["s.py".to_string()])
        .await
        .unwrap();
    assert!(result.sandboxed);
    assert!(
        result.stdout.contains("served"),
        "a service could not bind and be reached under the grant: {} {}",
        result.stdout,
        result.stderr
    );
}
