use crate::config::Config;
use crate::store::Store;
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const PROBE_INTERVAL: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
// only a TCP handshake to probe connectivity, sending no data; prefer AliDNS, fall back to Cloudflare
const PROBES: &[&str] = &["223.5.5.5:443", "1.1.1.1:443"];

/// consecutive failures -> network status: 0=ok, 1~2=flaky (unstable/recovering), >=3=down (confirmed offline)
pub fn classify(consecutive_failures: u32) -> &'static str {
    match consecutive_failures {
        0 => "ok",
        1 | 2 => "flaky",
        _ => "down",
    }
}

fn probe_once() -> bool {
    PROBES.iter().any(|addr| {
        addr.parse::<SocketAddr>()
            .ok()
            .and_then(|a| TcpStream::connect_timeout(&a, PROBE_TIMEOUT).ok())
            .is_some()
    })
}

pub fn start(store: Arc<Mutex<Store>>, config: Arc<Mutex<Config>>) {
    thread::spawn(move || {
        let mut failures: u32 = 0;
        loop {
            // the user can turn off network probing in settings (privacy / offline)
            let probe_on = config.lock().map(|c| c.network_probe).unwrap_or(true);
            if !probe_on {
                store.lock().unwrap().set_network("ok");
                thread::sleep(PROBE_INTERVAL);
                continue;
            }
            if probe_once() {
                failures = 0;
            } else {
                failures = failures.saturating_add(1);
            }
            store.lock().unwrap().set_network(classify(failures));
            thread::sleep(PROBE_INTERVAL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn classify_thresholds() {
        assert_eq!(classify(0), "ok");
        assert_eq!(classify(1), "flaky");
        assert_eq!(classify(2), "flaky");
        assert_eq!(classify(3), "down");
        assert_eq!(classify(9), "down");
    }
}
