//! The systemd watchdog keepalive: `WATCHDOG=1` only while liveness answers.
//!
//! systemd restarts a service on `Restart=on-failure` when the main process
//! exits, but a hung process never exits. With `WatchdogSec`, the service must
//! prove it is alive: the process sends `WATCHDOG=1` to the `NOTIFY_SOCKET`
//! before the deadline, and a missed keepalive makes systemd kill and restart
//! it. The keepalive is self-checked — the process probes its own
//! `/health/live` route over loopback and signals only on `HTTP/1.1 200`. The
//! database readiness route is never probed, so a database outage cannot
//! restart a service whose own machinery still works.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The parsed systemd watchdog contract. `None` from
/// [`Watchdog::from_environment`] means the process did not start under a
/// watchdog-configured unit and must not touch any notify socket.
pub struct Watchdog {
    /// systemd's notify socket: a filesystem path, or on Linux an abstract
    /// name (`NOTIFY_SOCKET` starting with `@`).
    socket: std::os::unix::net::SocketAddr,
    /// The systemd deadline (`WATCHDOG_USEC`): the longest gap between
    /// keepalives systemd tolerates before restarting the service.
    interval: Duration,
}

impl Watchdog {
    /// Read the watchdog contract from systemd's environment variables.
    ///
    /// `NOTIFY_SOCKET` and a positive integer `WATCHDOG_USEC` are required,
    /// and a set `WATCHDOG_PID` must name this process — anything else means
    /// "not watchdogged" and returns `None`, so a manually started process
    /// never blocks on a socket it does not have. A relative socket path is
    /// refused the same way: systemd always sets an absolute one.
    pub fn from_environment(vars: &HashMap<String, String>, pid: u32) -> Option<Watchdog> {
        let socket = vars.get("NOTIFY_SOCKET")?;
        let socket = if let Some(name) = socket.strip_prefix('@') {
            abstract_socket(name)?
        } else {
            let path = std::path::PathBuf::from(socket);
            if !path.is_absolute() {
                return None;
            }
            std::os::unix::net::SocketAddr::from_pathname(path).ok()?
        };
        if let Some(owner) = vars.get("WATCHDOG_PID")
            && owner.parse::<u32>().ok() != Some(pid)
        {
            return None;
        }
        Some(Watchdog {
            socket,
            interval: interval_from_usec(vars)?,
        })
    }

    /// Send one datagram to systemd's notify socket.
    ///
    /// A send failure (socket removed mid-run, queue full) is reported to the
    /// caller; the keepalive loop ignores it, because a single missed datagram
    /// costs at most one restart, and panicking would cause that restart.
    pub fn notify(&self, message: &str) -> std::io::Result<()> {
        let socket = std::os::unix::net::UnixDatagram::unbound()?;
        socket.send_to_addr(message.as_bytes(), &self.socket)?;
        Ok(())
    }

    /// Probe `/health/live` on `bind` and signal systemd while it answers.
    ///
    /// The loop probes every `interval / 2`, so a probe always lands well
    /// before the systemd deadline. Each probe sends one plain HTTP request
    /// over a fresh loopback connection with a total timeout of two seconds
    /// and reads at most 4 KiB — plenty for a status line. Only the exact
    /// `HTTP/1.1 200` status line counts as healthy; a 503, a hang or a
    /// refused connection sends nothing, and systemd restarts the service
    /// when the deadline passes without a keepalive.
    ///
    /// Errors from [`notify`](Self::notify) are ignored: a transient send
    /// failure costs at most one restart, and the loop must survive it.
    pub fn start(self, bind: SocketAddr) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(self.interval / 2);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if probe_live(bind).await {
                    let _ = self.notify("WATCHDOG=1");
                }
            }
        })
    }
}

/// The systemd deadline from `WATCHDOG_USEC`: a positive integer of
/// microseconds, or nothing when the variable is missing, zero, or not a
/// number.
fn interval_from_usec(vars: &HashMap<String, String>) -> Option<Duration> {
    let usec = vars
        .get("WATCHDOG_USEC")
        .and_then(|value| value.parse::<u64>().ok())?;
    if usec == 0 {
        return None;
    }
    Some(Duration::from_micros(usec))
}

/// The Linux abstract socket address for `name` (the `NOTIFY_SOCKET` value
/// after `@`). Abstract sockets do not exist elsewhere, so off Linux the
/// watchdog stays off.
#[cfg(target_os = "linux")]
fn abstract_socket(name: &str) -> Option<std::os::unix::net::SocketAddr> {
    use std::os::linux::net::SocketAddrExt;
    std::os::unix::net::SocketAddr::from_abstract_name(name).ok()
}

#[cfg(not(target_os = "linux"))]
fn abstract_socket(_name: &str) -> Option<std::os::unix::net::SocketAddr> {
    None
}

/// One liveness probe: `GET /health/live` with a two-second total timeout,
/// healthy only on an exact `HTTP/1.1 200` status line.
async fn probe_live(bind: SocketAddr) -> bool {
    let request = format!("GET /health/live HTTP/1.1\r\nHost: {bind}\r\nConnection: close\r\n\r\n");
    let probe = async {
        let mut stream = tokio::net::TcpStream::connect(bind).await?;
        stream.write_all(request.as_bytes()).await?;
        let mut answer = Vec::with_capacity(1024);
        let mut chunk = [0u8; 512];
        while answer.len() < 4096 {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            answer.extend_from_slice(&chunk[..read]);
            // The status line alone decides health; the headers after it are
            // never parsed.
            if answer.contains(&b'\n') {
                break;
            }
        }
        Ok::<bool, std::io::Error>(answer.starts_with(b"HTTP/1.1 200"))
    };
    tokio::time::timeout(Duration::from_secs(2), probe)
        .await
        .unwrap_or(Ok(false))
        .unwrap_or(false)
}
