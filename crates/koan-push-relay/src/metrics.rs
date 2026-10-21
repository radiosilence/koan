//! Counters, in Prometheus' text format, on the metrics listener. Counts
//! only: nothing about which device or which server.

use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy)]
pub enum Route {
    Register,
    Revoke,
    Push,
}

const ROUTES: [&str; 3] = ["register", "revoke", "push"];

#[derive(Debug, Clone, Copy)]
pub enum Outcome {
    Ok,
    BadRequest,
    Gone,
    Limited,
    Unavailable,
}

const OUTCOMES: [&str; 5] = ["ok", "bad_request", "gone", "limited", "unavailable"];

#[derive(Default)]
pub struct Metrics {
    requests: [[AtomicU64; 5]; 3],
    /// Pushes by kind (wake, wake-now, summon, notify) that reached Apple.
    sent: [AtomicU64; 4],
}

impl Metrics {
    pub fn count(&self, route: Route, outcome: Outcome) {
        self.requests[route as usize][outcome as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub fn sent(&self, kind: crate::apns::Kind) {
        self.sent[kind as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub fn render(&self, gauges: &[(&str, &str, u64)]) -> String {
        let mut out = String::new();
        out.push_str("# HELP koan_relay_requests_total Requests by route and outcome.\n");
        out.push_str("# TYPE koan_relay_requests_total counter\n");
        for (r, route) in ROUTES.iter().enumerate() {
            for (o, outcome) in OUTCOMES.iter().enumerate() {
                let n = self.requests[r][o].load(Ordering::Relaxed);
                let _ = writeln!(
                    out,
                    "koan_relay_requests_total{{route=\"{route}\",outcome=\"{outcome}\"}} {n}"
                );
            }
        }
        out.push_str("# HELP koan_relay_pushes_sent_total Pushes Apple accepted, by kind.\n");
        out.push_str("# TYPE koan_relay_pushes_sent_total counter\n");
        for (k, kind) in ["wake", "wake-now", "summon", "notify"].iter().enumerate() {
            let n = self.sent[k].load(Ordering::Relaxed);
            let _ = writeln!(out, "koan_relay_pushes_sent_total{{kind=\"{kind}\"}} {n}");
        }
        for (name, help, value) in gauges {
            let _ = writeln!(
                out,
                "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}"
            );
        }
        out
    }
}
