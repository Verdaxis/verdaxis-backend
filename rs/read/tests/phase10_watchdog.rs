//! Phase 10 Task: the systemd watchdog keepalive.
//!
//! Every case is offline: a scratch notify socket, raw loopback HTTP servers,
//! and the committed unit files. Nothing here contacts a real service.
use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UnixDatagram};
use verdaxis_read::runtime::watchdog::Watchdog;

fn repository() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn environment(vars: &[(&str, &str)]) -> HashMap<String, String> {
    vars.iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn scratch_socket(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "verdaxis-p10-watchdog-{label}-{}.sock",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

/// A raw loopback HTTP server that answers every request with `status`.
async fn answer_with(status: &'static str) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let bind = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut request = vec![0u8; 1024];
                let _ = stream.read(&mut request).await;
                let answer = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n");
                let _ = stream.write_all(answer.as_bytes()).await;
            });
        }
    });
    (bind, server)
}

/// A loopback server that accepts connections and never answers, so a probe
/// hangs until its own two-second timeout.
async fn accept_and_hang() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let bind = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let (_read, _write) = stream.into_split();
                std::future::pending::<()>().await;
            });
        }
    });
    (bind, server)
}

/// Start the watchdog against a scratch notify socket with a 200 ms deadline,
/// so keepalives go out every 100 ms. The socket is returned for receiving.
async fn started_watchdog(
    label: &str,
    bind: std::net::SocketAddr,
) -> (UnixDatagram, tokio::task::JoinHandle<()>) {
    let path = scratch_socket(label);
    let receiver = UnixDatagram::bind(&path).expect("bind the scratch notify socket");
    let watchdog = Watchdog::from_environment(
        &environment(&[
            ("NOTIFY_SOCKET", path.to_str().unwrap()),
            ("WATCHDOG_USEC", "200000"),
            ("WATCHDOG_PID", &std::process::id().to_string()),
        ]),
        std::process::id(),
    )
    .expect("the environment is a valid watchdog contract");
    (receiver, watchdog.start(bind))
}

#[tokio::test(flavor = "multi_thread")]
async fn watchdog_sends_keepalives_while_liveness_answers() {
    let (bind, server) = answer_with("200 OK").await;
    let (receiver, task) = started_watchdog("answers", bind).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut received = 0_usize;
    while received < 2 && std::time::Instant::now() < deadline {
        let mut buffer = [0u8; 64];
        let Ok(datagram) = tokio::time::timeout(
            deadline.saturating_duration_since(std::time::Instant::now()),
            receiver.recv_from(&mut buffer),
        )
        .await
        else {
            break;
        };
        let (count, _) = datagram.expect("receive a keepalive");
        assert_eq!(&buffer[..count], b"WATCHDOG=1");
        received += 1;
    }
    task.abort();
    server.abort();
    assert!(received >= 2, "only {received} keepalives in 2 s");
}

#[tokio::test(flavor = "multi_thread")]
async fn watchdog_stops_when_liveness_fails_or_hangs() {
    let (failing, failing_server) = answer_with("503 Service Unavailable").await;
    let (hanging, hanging_server) = accept_and_hang().await;
    for (label, bind) in [("failing", failing), ("hanging", hanging)] {
        let path = scratch_socket(label);
        let receiver = UnixDatagram::bind(&path).expect("bind the scratch notify socket");
        let watchdog = Watchdog::from_environment(
            &environment(&[
                ("NOTIFY_SOCKET", path.to_str().unwrap()),
                ("WATCHDOG_USEC", "200000"),
            ]),
            std::process::id(),
        )
        .expect("the environment is a valid watchdog contract");
        let task = watchdog.start(bind);
        // Two keepalive periods pass with a 503 route or a hung route: the
        // keepalive loop sends nothing.
        let mut buffer = [0u8; 64];
        let quiet =
            tokio::time::timeout(Duration::from_millis(400), receiver.recv_from(&mut buffer)).await;
        task.abort();
        assert!(quiet.is_err(), "{label}: a keepalive arrived: {quiet:?}");
    }
    failing_server.abort();
    hanging_server.abort();
}

#[test]
fn watchdog_is_off_without_systemd_settings() {
    let pid = std::process::id().to_string();
    let without_socket = environment(&[("WATCHDOG_USEC", "30000000")]);
    let without_usec = environment(&[("NOTIFY_SOCKET", "/run/notify.sock")]);
    let zero_usec = environment(&[
        ("NOTIFY_SOCKET", "/run/notify.sock"),
        ("WATCHDOG_USEC", "0"),
    ]);
    let non_numeric_usec = environment(&[
        ("NOTIFY_SOCKET", "/run/notify.sock"),
        ("WATCHDOG_USEC", "soon"),
    ]);
    let relative_socket = environment(&[
        ("NOTIFY_SOCKET", "notify.sock"),
        ("WATCHDOG_USEC", "30000000"),
    ]);
    let other_pid = environment(&[
        ("NOTIFY_SOCKET", "/run/notify.sock"),
        ("WATCHDOG_USEC", "30000000"),
        ("WATCHDOG_PID", "1"),
    ]);
    let absolute = environment(&[
        ("NOTIFY_SOCKET", "/run/notify.sock"),
        ("WATCHDOG_USEC", "30000000"),
        ("WATCHDOG_PID", &pid),
    ]);
    assert!(Watchdog::from_environment(&without_socket, std::process::id()).is_none());
    assert!(Watchdog::from_environment(&without_usec, std::process::id()).is_none());
    assert!(Watchdog::from_environment(&zero_usec, std::process::id()).is_none());
    assert!(Watchdog::from_environment(&non_numeric_usec, std::process::id()).is_none());
    assert!(Watchdog::from_environment(&relative_socket, std::process::id()).is_none());
    assert!(Watchdog::from_environment(&other_pid, std::process::id()).is_none());
    assert!(Watchdog::from_environment(&absolute, std::process::id()).is_some());
}

/// Both serve units arm the watchdog, so the keepalive task runs in every
/// deployed environment.
#[test]
fn serve_units_enable_the_watchdog() {
    for name in ["verdaxis-rust.service", "verdaxis-rust-staging.service"] {
        let unit = std::fs::read_to_string(repository().join("deploy/systemd").join(name))
            .unwrap_or_else(|error| panic!("read {name}: {error}"));
        assert!(unit.contains("WatchdogSec=30s"), "{name}");
        assert!(unit.contains("NotifyAccess=main"), "{name}");
        assert!(unit.contains("Restart=on-failure"), "{name}");
    }
}
