//! A TCP proxy that injects network faults between a server and a dependency
//! (Redis, PostgreSQL) or between a client and a server.
//!
//! Each server instance gets its own proxy, so a fault can be aimed at one
//! instance while the others keep a healthy path to the same real service.
//! Faults act on whole connections:
//!
//! - [`ChaosProxy::refuse`]: close every connection and refuse new ones — a
//!   dependency that is down.
//! - [`ChaosProxy::blackhole`]: keep connections open but drop every byte, both
//!   ways — a network partition. Nothing is closed, so only a timeout or a
//!   liveness check notices.
//! - [`ChaosProxy::strand_existing`]: like `blackhole`, but only for the
//!   connections open right now, forever; new connections work. Models a
//!   middlebox that lost its state for a long-lived connection without telling
//!   either end.
//! - [`ChaosProxy::sever`]: close every open connection once and keep
//!   accepting — a dropped link.
//! - [`ChaosProxy::heal`]: end `refuse`/`blackhole`. Connections that were
//!   blackholed are closed, since bytes were lost mid-stream; stranded
//!   connections stay stranded.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Pass,
    Refuse,
    Blackhole,
}

#[derive(Clone, Copy, Debug)]
struct Faults {
    mode: Mode,
    /// Connections with an id below this are closed.
    closed_below: u64,
    /// Connections with an id below this drop every byte, forever.
    stranded_below: u64,
}

pub struct ChaosProxy {
    pub port: u16,
    upstream: String,
    faults: watch::Sender<Faults>,
    next_id: Arc<AtomicU64>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ChaosProxy {
    fn drop(&mut self) {
        self.task.abort();
        // Close every proxied connection with the proxy.
        self.faults.send_modify(|f| {
            f.mode = Mode::Refuse;
            f.closed_below = u64::MAX;
        });
    }
}

impl ChaosProxy {
    /// Starts a proxy on a free local port forwarding to `upstream`
    /// (`host:port`).
    pub async fn start(upstream: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (faults, _) = watch::channel(Faults {
            mode: Mode::Pass,
            closed_below: 0,
            stranded_below: 0,
        });
        let next_id = Arc::new(AtomicU64::new(0));
        let task = {
            let faults = faults.clone();
            let next_id = Arc::clone(&next_id);
            let upstream = upstream.to_string();
            tokio::spawn(async move {
                loop {
                    let Ok((client, _)) = listener.accept().await else {
                        continue;
                    };
                    let id = next_id.fetch_add(1, Ordering::SeqCst);
                    if faults.borrow().mode == Mode::Refuse {
                        drop(client);
                        continue;
                    }
                    let rx = faults.subscribe();
                    let upstream = upstream.clone();
                    tokio::spawn(async move {
                        let Ok(server) = TcpStream::connect(&upstream).await else {
                            return;
                        };
                        let _ = client.set_nodelay(true);
                        let _ = server.set_nodelay(true);
                        let (cr, cw) = client.into_split();
                        let (sr, sw) = server.into_split();
                        tokio::select! {
                            _ = pump(id, cr, sw, rx.clone()) => {}
                            _ = pump(id, sr, cw, rx) => {}
                        }
                    });
                }
            })
        };
        ChaosProxy {
            port,
            upstream: upstream.to_string(),
            faults,
            next_id,
            task,
        }
    }

    /// Starts a proxy for the host and port of `url`, returning it with `url`
    /// rewritten to go through it.
    pub async fn for_url(url: &str) -> (Self, String) {
        let mut parsed = reqwest::Url::parse(url).unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port_or_known_default().unwrap();
        let proxy = Self::start(&format!("{host}:{port}")).await;
        parsed.set_host(Some("127.0.0.1")).unwrap();
        parsed.set_port(Some(proxy.port)).unwrap();
        (proxy, parsed.to_string())
    }

    pub fn refuse(&self) {
        let id = self.next_id.load(Ordering::SeqCst);
        self.faults.send_modify(|f| {
            f.mode = Mode::Refuse;
            f.closed_below = id;
        });
    }

    pub fn blackhole(&self) {
        self.faults.send_modify(|f| f.mode = Mode::Blackhole);
    }

    pub fn strand_existing(&self) {
        let id = self.next_id.load(Ordering::SeqCst);
        self.faults.send_modify(|f| f.stranded_below = id);
    }

    pub fn sever(&self) {
        let id = self.next_id.load(Ordering::SeqCst);
        self.faults.send_modify(|f| f.closed_below = id);
    }

    pub fn heal(&self) {
        let id = self.next_id.load(Ordering::SeqCst);
        self.faults.send_modify(|f| {
            if f.mode == Mode::Blackhole {
                f.closed_below = id;
            }
            f.mode = Mode::Pass;
        });
    }

    pub fn upstream(&self) -> &str {
        &self.upstream
    }
}

/// Copies one direction of a connection, applying the current faults to
/// every chunk. Returns when either side closes or the connection is closed
/// by a fault.
async fn pump(
    id: u64,
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    mut faults: watch::Receiver<Faults>,
) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        if id < faults.borrow().closed_below {
            return;
        }
        tokio::select! {
            read = from.read(&mut buf) => {
                let n = match read {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let f = *faults.borrow();
                if id < f.closed_below {
                    return;
                }
                if id < f.stranded_below || f.mode == Mode::Blackhole {
                    continue;
                }
                if to.write_all(&buf[..n]).await.is_err() {
                    return;
                }
            }
            changed = faults.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}
