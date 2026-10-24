//! QUIC settings both ends agree on.

use std::time::Duration;

/// Keep-alives every 2 s hold Wi-Fi and LTE NAT bindings open; 20 s of silence ends the session.
pub fn transport() -> quinn::TransportConfig {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(2)));
    t.max_idle_timeout(Some(
        Duration::from_secs(20).try_into().expect("idle timeout fits a varint"),
    ));
    t
}
