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

/// How many ports there are to hand out.
const PORTS: u32 = 10_000;

/// Where the ports [`free_port`] hands out start: the [`PORTS`] right below
/// the range the OS takes the local port of an outgoing connection from, so no
/// client connection in another test can be holding one.
///
/// Linux lets a host move that range and says where it is, so it is read
/// there, and the ports go above it when there is no room below. macOS and
/// Windows are taken at their defaults, from 49152, as is a Linux with no room
/// on either side.
fn first_port() -> u16 {
    const DEFAULT: u16 = 20_000;

    let Ok(range) = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range") else {
        return DEFAULT;
    };
    let mut bounds = range.split_whitespace().map(str::parse::<u32>);
    let (Some(Ok(low)), Some(Ok(high))) = (bounds.next(), bounds.next()) else {
        return DEFAULT;
    };
    // Below 1024 a port is a privileged one.
    let first = if low >= 1_024 + PORTS {
        low - PORTS
    } else if high + PORTS <= u32::from(u16::MAX) {
        high + 1
    } else {
        return DEFAULT;
    };
    u16::try_from(first).unwrap_or(DEFAULT)
}

/// A port on `127.0.0.1` that nothing else in this process has been handed,
/// and that was free a moment ago.
///
/// Asking the OS for port 0 and letting it go hands back a port from the range
/// it then gives to outgoing connections, and by the time the server binds it
/// a client in another test can hold it. These come one at a time from beside
/// that range ([`first_port`]), starting where this process's id puts them, so
/// that test processes run side by side start apart. One already taken, by
/// anything, is skipped.
pub(crate) fn free_port() -> u16 {
    static NEXT: OnceLock<(u16, AtomicU32)> = OnceLock::new();
    let (first, next) = NEXT.get_or_init(|| {
        let start = std::process::id().wrapping_mul(7_919);
        (first_port(), AtomicU32::new(start))
    });

    for _ in 0..PORTS {
        let offset = next.fetch_add(1, Ordering::Relaxed) % PORTS;
        let port = first + u16::try_from(offset).expect("an offset below PORTS");
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("no free port from {first} on");
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
