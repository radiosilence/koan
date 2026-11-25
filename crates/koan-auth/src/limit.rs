//! Limits on what one address, or one account, may try in a minute.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Sign-in attempts one address may make in [`LOGIN_WINDOW_SECS`].
///
/// argon2 is tuned to cost ~19 MiB and real CPU per check, which is right for
/// resisting cracking and ruinous when anyone may trigger it at will: a few
/// hundred concurrent sign-ins exhaust memory and starve every other request.
/// The window is coarse on purpose; it bounds cost, it is not a quota.
pub const LOGIN_MAX_PER_WINDOW: u32 = 10;
pub const LOGIN_WINDOW_SECS: u64 = 60;
/// Above this many tracked keys, lapsed windows go before more are added.
const TRACKED_MAX: usize = 4096;

/// Windows by key. Once the map holds more than [`TRACKED_MAX`], lapsed ones
/// are swept each time it doubles, and at least once a window. A sweep scans
/// the whole map under the limiter's lock, so sweeping on every insert while
/// a flood of fresh keys (an IPv6 /48 holds 65536 networks) kept it full would
/// make each request pay for every key tracked; sweeping once a window as well
/// lets the map shrink again once a flood has passed.
struct Windows<K, W> {
    map: HashMap<K, W>,
    sweep_at: usize,
    swept: Instant,
}

impl<K: std::hash::Hash + Eq, W> Default for Windows<K, W> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            sweep_at: TRACKED_MAX,
            swept: Instant::now(),
        }
    }
}

impl<K: std::hash::Hash + Eq, W> Windows<K, W> {
    fn sweep(&mut self, window: Duration, lapsed: impl Fn(&W) -> bool) {
        let len = self.map.len();
        if len > self.sweep_at || (len > TRACKED_MAX && self.swept.elapsed() >= window) {
            self.map.retain(|_, w| !lapsed(w));
            self.sweep_at = (self.map.len() * 2).max(TRACKED_MAX);
            self.swept = Instant::now();
        }
    }
}

/// Failed password sign-ins allowed for one username in a minute, from every
/// address together. Per-address limits do not stop a guesser with many
/// addresses; this does, at a rate no person typing would reach.
pub const FAILURES_PER_USERNAME_PER_MINUTE: u32 = 60;
pub const FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// A fixed-window cap on requests per address (see [`network`]).
pub struct RateLimiter {
    windows: Mutex<Windows<IpAddr, (u64, u32)>>,
    window_secs: u64,
    max: u32,
}

impl Default for RateLimiter {
    /// The cap on sign-in attempts.
    fn default() -> Self {
        Self::new(LOGIN_WINDOW_SECS, LOGIN_MAX_PER_WINDOW)
    }
}

impl RateLimiter {
    pub fn new(window_secs: u64, max: u32) -> Self {
        Self {
            windows: Mutex::default(),
            window_secs,
            max,
        }
    }

    /// Counts a request from `ip`, and says whether its network is still
    /// within its allowance for the current window.
    pub fn allow(&self, ip: IpAddr) -> bool {
        let now = crate::clock::unix_secs() as u64;
        let mut windows = self.windows.lock();
        windows.sweep(Duration::from_secs(self.window_secs), |(start, _)| {
            now.saturating_sub(*start) >= self.window_secs
        });
        let entry = windows.map.entry(network(ip)).or_insert((now, 0));
        if now.saturating_sub(entry.0) >= self.window_secs {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= self.max
    }
}

/// Failures by key, in fixed windows of [`FAILURE_WINDOW`].
pub struct FailureLimiter<K> {
    limit: u32,
    windows: Mutex<Windows<K, (Instant, u32)>>,
}

impl<K: std::hash::Hash + Eq> FailureLimiter<K> {
    pub fn new(limit: u32) -> Self {
        Self {
            limit,
            windows: Default::default(),
        }
    }

    pub fn exhausted(&self, key: &K) -> bool {
        let windows = self.windows.lock();
        windows
            .map
            .get(key)
            .is_some_and(|(start, count)| start.elapsed() < FAILURE_WINDOW && *count >= self.limit)
    }

    pub fn record(&self, key: K) {
        let mut windows = self.windows.lock();
        windows.sweep(FAILURE_WINDOW, |(start, _)| {
            start.elapsed() >= FAILURE_WINDOW
        });
        let entry = windows.map.entry(key).or_insert((Instant::now(), 0));
        if entry.0.elapsed() >= FAILURE_WINDOW {
            *entry = (Instant::now(), 0);
        }
        entry.1 += 1;
    }
}

/// The network an address counts against a per-address limit as: itself, or
/// for IPv6 its /64, which one subscriber is usually given whole. Keyed on the
/// full address, a client would have 2^64 fresh allowances.
pub fn network(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => IpAddr::V6((u128::from(v6) & !((1u128 << 64) - 1)).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_gets_its_allowance_and_no_more() {
        let limiter = RateLimiter::new(60, 3);
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        assert!((0..3).all(|_| limiter.allow(ip)));
        assert!(!limiter.allow(ip));
        assert!(limiter.allow("203.0.113.10".parse().unwrap()));
    }

    #[test]
    fn an_ipv6_subscriber_is_one_network() {
        let limiter = RateLimiter::new(60, 1);
        assert!(limiter.allow("2001:db8:1:2::1".parse().unwrap()));
        assert!(!limiter.allow("2001:db8:1:2:ffff::9".parse().unwrap()));
        assert!(limiter.allow("2001:db8:1:3::1".parse().unwrap()));
        assert_eq!(
            network("::ffff:192.0.2.1".parse().unwrap()),
            "192.0.2.1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn a_flood_of_fresh_addresses_is_swept_as_it_doubles_not_on_every_request() {
        let limiter = RateLimiter::new(60, 1);
        let host = |n: usize| IpAddr::V6(((0x2001_0db8_u128 << 96) | ((n as u128) << 64)).into());
        for n in 0..=TRACKED_MAX {
            limiter.allow(host(n));
        }
        // Nothing has lapsed: the sweep that found the map full kept
        // everything, and waits now for it to double.
        limiter.allow(host(TRACKED_MAX + 1));
        let swept_at = limiter.windows.lock().sweep_at;
        assert_eq!(swept_at, 2 * (TRACKED_MAX + 1));
        for n in TRACKED_MAX + 2..swept_at {
            limiter.allow(host(n));
        }
        assert_eq!(limiter.windows.lock().sweep_at, swept_at);
        // Still limited all the while.
        assert!(!limiter.allow(host(0)));
    }

    #[test]
    fn a_passed_flood_is_swept_within_a_window() {
        // A monotonic clock younger than the window cannot say "a window ago".
        let Some(long_ago) = Instant::now().checked_sub(FAILURE_WINDOW) else {
            return;
        };
        let failures = FailureLimiter::new(1);
        {
            // After a flood: lapsed windows, and a sweep put off until the
            // map doubles from the size the flood reached.
            let mut windows = failures.windows.lock();
            for n in 0..=TRACKED_MAX {
                windows.map.insert(n, (long_ago, 1));
            }
            windows.sweep_at = 64 * TRACKED_MAX;
            windows.swept = long_ago;
        }
        failures.record(usize::MAX);
        let windows = failures.windows.lock();
        assert_eq!(windows.map.len(), 1);
        assert_eq!(windows.sweep_at, TRACKED_MAX);
    }

    #[test]
    fn failures_count_up_to_the_limit() {
        let failures = FailureLimiter::new(2);
        failures.record("ann");
        assert!(!failures.exhausted(&"ann"));
        failures.record("ann");
        assert!(failures.exhausted(&"ann"));
        assert!(!failures.exhausted(&"bob"));
    }
}
