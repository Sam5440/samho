//! Test/fixture networking helpers.
//!
//! Integration tests in this workspace intentionally use only loopback ports
//! 1300 through 1400. `bind_loopback` reserves a port by returning the listener;
//! callers should pass that listener to their HTTP server task.

use std::io;
use std::sync::atomic::{AtomicU16, Ordering};
use tokio::net::TcpListener;

const FIRST_PORT: u16 = 1301;
const LAST_PORT: u16 = 1399;
static NEXT_PORT: AtomicU16 = AtomicU16::new(FIRST_PORT);

/// Bind an available loopback listener from the test-only 1300-1400 range.
///
/// A fixed range is required by the repository's test policy. Ports are tried
/// in a round-robin order and the successful listener remains bound, so two
/// concurrent fixtures cannot select the same endpoint.
pub async fn bind_loopback() -> io::Result<TcpListener> {
    let span = LAST_PORT - FIRST_PORT + 1;
    let start = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
    for offset in 0..span {
        let port = FIRST_PORT + (start - FIRST_PORT + offset) % span;
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AddrInUse,
        "no loopback fixture port available in 1301-1399",
    ))
}

/// Returns the inclusive policy bounds for diagnostics and tests.
pub const fn port_range() -> (u16, u16) {
    (FIRST_PORT, LAST_PORT)
}
