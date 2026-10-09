//! What the integration tests share: a port for each server they start, and a
//! wait for it to come up.
//!
//! Each test that uses it declares `mod common;`. Cargo builds this file as a
//! test target of its own as well, one with no tests, which is all the gate
//! below is for: without an HTTP transport there is no server to wait for.
#![cfg(any(feature = "http-server-volga", feature = "http-client"))]
#![allow(dead_code, reason = "each test binary uses a part of it")]

use std::net::TcpListener;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// The first of the ports [`free_port`] hands out.
///
/// They are below the range every common OS takes the local port of an
/// outgoing connection from by default (from 32768 on Linux, from 49152 on
/// macOS and Windows), so no client connection in another test can be holding
/// one. A host that moves that range over them brings the race back, and
/// loudly: a client's socket takes no connections, so [`serving`] reports the
/// server that could not bind instead of a test talking to something else.
const FIRST_PORT: u16 = 20_000;

/// How many ports there are to hand out, from [`FIRST_PORT`].
const PORTS: u32 = 10_000;

/// A port on `127.0.0.1` that nothing else in this process has been handed,
/// and that was free a moment ago.
///
/// Asking the OS for port 0 and letting it go hands back a port from the range
/// it then gives to outgoing connections, and by the time the server binds it
/// a client in another test can hold it. These come one at a time from a range
/// the OS does not give out, starting where this process's id puts it, so that
/// test processes run side by side start apart. One already taken, by anything,
/// is skipped.
pub(crate) fn free_port() -> u16 {
    static NEXT: OnceLock<AtomicU32> = OnceLock::new();
    let next = NEXT.get_or_init(|| AtomicU32::new(std::process::id().wrapping_mul(7_919)));

    for _ in 0..PORTS {
        let offset = next.fetch_add(1, Ordering::Relaxed) % PORTS;
        let port = FIRST_PORT + u16::try_from(offset).expect("an offset below PORTS");
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("no free port from {FIRST_PORT} on");
}

/// Waits for the server `server` runs to take connections at `addr`.
///
/// Fails as soon as the server stops, most often for a port it could not bind,
/// rather than waiting out the deadline. It cannot tell its server from
/// another listener on the same port; [`free_port`] is what keeps there from
/// being one.
pub(crate) async fn serving<T>(addr: &str, server: &tokio::task::JoinHandle<T>) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            !server.is_finished(),
            "the server at {addr} stopped before it took a connection"
        );
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the server at {addr} never came up"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
