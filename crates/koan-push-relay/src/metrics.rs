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
const KINDS: usize = crate::apns::Kind::ALL.len();

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
    requests: [[AtomicU64; OUTCOMES.len()]; ROUTES.len()],
    /// Pushes Apple accepted, by kind.
    sent: [AtomicU64; KINDS],
    /// Proofs Apple accepted, and refused as no device of this app's.
    proofs: [AtomicU64; 2],
}

impl Metrics {
    pub fn count(&self, route: Route, outcome: Outcome) {
        self.requests[route as usize][outcome as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub fn sent(&self, kind: crate::apns::Kind) {
        self.sent[kind as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub fn proof(&self, accepted: bool) {
        self.proofs[usize::from(!accepted)].fetch_add(1, Ordering::Relaxed);
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
        for kind in crate::apns::Kind::ALL {
            let n = self.sent[kind as usize].load(Ordering::Relaxed);
            let _ = writeln!(
                out,
                "koan_relay_pushes_sent_total{{kind=\"{}\"}} {n}",
                kind.name()
            );
        }
        out.push_str("# HELP koan_relay_proofs_total Proofs of a token, by Apple's answer.\n");
        out.push_str("# TYPE koan_relay_proofs_total counter\n");
        for (i, answer) in ["accepted", "refused"].iter().enumerate() {
            let n = self.proofs[i].load(Ordering::Relaxed);
            let _ = writeln!(out, "koan_relay_proofs_total{{answer=\"{answer}\"}} {n}");
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
