//! Rate limits, as GCRA: each caller is one timestamp, the theoretical time
//! its next request is due, so a limit costs sixteen bytes of key and eight of
//! state per caller and no timers.
//!
//! A limit of `n` per `period` allows a burst of `n` and then one every
//! `period / n`.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Nanoseconds since the relay started, which every limit counts in.
#[derive(Clone, Copy)]
pub struct Clock(Instant);

impl Clock {
    pub fn new() -> Self {
        Self(Instant::now())
    }

    pub fn now(&self) -> u64 {
        self.0.elapsed().as_nanos() as u64
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Rate {
    /// Time between requests at the sustained rate.
    interval: u64,
    /// How far ahead of now a caller's due time may run: the burst.
    tolerance: u64,
}

impl Rate {
    pub fn per(n: u32, period: Duration) -> Self {
        let n = u64::from(n.max(1));
        let interval = (period.as_nanos() as u64 / n).max(1);
        Self {
            interval,
            tolerance: interval * (n - 1),
        }
    }

    /// Admit a request due at `*tat`, moving it on; or say how long until one
    /// would be admitted.
    fn admit(&self, tat: &mut u64, now: u64) -> Result<(), Duration> {
        let due = (*tat).max(now);
        let ahead = due - now;
        if ahead > self.tolerance {
            return Err(Duration::from_nanos(ahead - self.tolerance));
        }
        *tat = due + self.interval;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Over the limit; try again after this.
    Limited(Duration),
    /// Too many callers are being counted to start counting another.
    Full,
}

const SHARDS: usize = 64;

/// How often a full shard may be swept to make room. A sweep walks the whole
/// shard under its lock, so a flood of new callers must not run one each.
const FULL_SWEEP_EVERY: u64 = 1_000_000_000;

struct Shard<K> {
    map: HashMap<K, u64>,
    /// When the shard was last swept because it was full.
    swept: Option<u64>,
}

/// One limit per key, sharded so that callers contend only within a shard.
pub struct Keyed<K> {
    rate: Rate,
    shards: Vec<Mutex<Shard<K>>>,
    hasher: std::collections::hash_map::RandomState,
    per_shard: usize,
    len: AtomicUsize,
}

impl<K: Hash + Eq + Copy> Keyed<K> {
    pub fn new(rate: Rate, max_tracked: usize) -> Self {
        Self {
            rate,
            shards: (0..SHARDS)
                .map(|_| {
                    Mutex::new(Shard {
                        map: HashMap::new(),
                        swept: None,
                    })
                })
                .collect(),
            hasher: Default::default(),
            per_shard: max_tracked.div_ceil(SHARDS).max(1),
            len: AtomicUsize::new(0),
        }
    }

    pub fn check(&self, key: K, now: u64) -> Result<(), Refused> {
        let shard = &self.shards[self.hasher.hash_one(key) as usize % SHARDS];
        let mut shard = shard.lock();
        if let Some(tat) = shard.map.get_mut(&key) {
            return self.rate.admit(tat, now).map_err(Refused::Limited);
        }
        if shard.map.len() >= self.per_shard {
            if shard
                .swept
                .is_some_and(|at| now.saturating_sub(at) < FULL_SWEEP_EVERY)
            {
                return Err(Refused::Full);
            }
            shard.swept = Some(now);
            let before = shard.map.len();
            shard.map.retain(|_, tat| *tat > now);
            self.len
                .fetch_sub(before - shard.map.len(), Ordering::Relaxed);
            if shard.map.len() >= self.per_shard {
                return Err(Refused::Full);
            }
        }
        let mut tat = 0;
        self.rate.admit(&mut tat, now).map_err(Refused::Limited)?;
        shard.map.insert(key, tat);
        self.len.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Hand back one request's allowance, for one that was admitted and
    /// then not carried out.
    pub fn refund(&self, key: &K) {
        let shard = &self.shards[self.hasher.hash_one(key) as usize % SHARDS];
        if let Some(tat) = shard.lock().map.get_mut(key) {
            *tat = tat.saturating_sub(self.rate.interval);
        }
    }

    /// Forget callers whose limit has fully recovered: to the limit, they
    /// are the same as callers never seen.
    pub fn sweep(&self, now: u64) {
        for shard in &self.shards {
            let map = &mut shard.lock().map;
            let before = map.len();
            map.retain(|_, tat| *tat > now);
            self.len.fetch_sub(before - map.len(), Ordering::Relaxed);
            if map.capacity() > 4 * map.len().max(64) {
                map.shrink_to_fit();
            }
        }
    }

    pub fn tracked(&self) -> usize {
        self.len.load(Ordering::Relaxed)
    }
}

/// One limit shared by everyone, without a lock.
pub struct Global {
    rate: Rate,
    tat: AtomicU64,
}

impl Global {
    pub fn new(rate: Rate) -> Self {
        Self {
            rate,
            tat: AtomicU64::new(0),
        }
    }

    /// Hand back one request's allowance.
    pub fn refund(&self) {
        let mut current = self.tat.load(Ordering::Relaxed);
        loop {
            let tat = current.saturating_sub(self.rate.interval);
            match self
                .tat
                .compare_exchange_weak(current, tat, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(seen) => current = seen,
            }
        }
    }

    pub fn check(&self, now: u64) -> Result<(), Duration> {
        let mut current = self.tat.load(Ordering::Relaxed);
        loop {
            let mut tat = current;
            self.rate.admit(&mut tat, now)?;
            match self
                .tat
                .compare_exchange_weak(current, tat, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return Ok(()),
                Err(seen) => current = seen,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: u64 = 1_000_000_000;

    #[test]
    fn a_burst_is_allowed_then_one_per_interval() {
        let limit = Keyed::new(Rate::per(3, Duration::from_secs(3)), 100);
        for _ in 0..3 {
            assert!(limit.check(1u8, 0).is_ok());
        }
        assert_eq!(
            limit.check(1, 0),
            Err(Refused::Limited(Duration::from_secs(1)))
        );
        assert!(limit.check(1, SEC).is_ok());
        assert!(limit.check(1, SEC).is_err());
        assert!(
            limit.check(2, 0).is_ok(),
            "another caller has its own limit"
        );
    }

    #[test]
    fn a_caller_idle_for_a_period_has_its_whole_burst_back() {
        let limit = Keyed::new(Rate::per(2, Duration::from_secs(60)), 100);
        assert!(limit.check(1u8, 0).is_ok());
        assert!(limit.check(1, 0).is_ok());
        assert!(limit.check(1, 0).is_err());
        assert!(limit.check(1, 60 * SEC).is_ok());
        assert!(limit.check(1, 60 * SEC).is_ok());
        assert!(limit.check(1, 60 * SEC).is_err());
    }

    #[test]
    fn a_refund_hands_one_request_back() {
        let limit = Keyed::new(Rate::per(2, Duration::from_secs(60)), 100);
        assert!(limit.check(1u8, 0).is_ok());
        assert!(limit.check(1, 0).is_ok());
        assert!(limit.check(1, 0).is_err());
        limit.refund(&1);
        assert!(limit.check(1, 0).is_ok());
        assert!(limit.check(1, 0).is_err());
        limit.refund(&9);
        assert_eq!(
            limit.tracked(),
            1,
            "refunding an unknown caller adds nothing"
        );
    }

    #[test]
    fn a_full_limiter_sweeps_then_refuses() {
        let limit = Keyed::new(Rate::per(1, Duration::from_secs(10)), SHARDS);
        let mut accepted = 0;
        let mut refused = 0;
        for key in 0..10_000u32 {
            match limit.check(key, 0) {
                Ok(()) => accepted += 1,
                Err(Refused::Full) => refused += 1,
                Err(e) => panic!("{e:?}"),
            }
        }
        assert!(accepted <= SHARDS);
        assert!(refused > 0);
        assert_eq!(limit.tracked(), accepted);
        // Ten seconds on, every caller has recovered: the first newcomer to a
        // full shard sweeps it, and makes room.
        assert!(limit.check(99_999, 10 * SEC).is_ok());
        limit.sweep(10 * SEC);
        assert!(limit.tracked() <= 1);
    }

    #[test]
    fn a_full_shard_is_swept_at_most_once_a_second() {
        let limit = Keyed::new(Rate::per(1, Duration::from_secs(10)), SHARDS);
        // Fill every shard with callers who recover at ten seconds.
        let mut key = 0u32;
        while limit.tracked() < SHARDS {
            let _ = limit.check(key, 0);
            key += 1;
        }
        // At five seconds nothing has recovered: a newcomer sweeps in vain,
        // and a newcomer to the same shard within the second does not sweep.
        assert_eq!(limit.check(key, 5 * SEC), Err(Refused::Full));
        let swept_at = |k: u32| {
            let shard = &limit.shards[limit.hasher.hash_one(k) as usize % SHARDS];
            shard.lock().swept
        };
        assert_eq!(swept_at(key), Some(5 * SEC));
        // Past ten seconds the callers have recovered, but the shard was swept
        // under a second ago, so it refuses without sweeping again.
        let same_shard = (key + 1..)
            .find(|k| swept_at(*k) == Some(5 * SEC) && *k != key)
            .unwrap();
        assert_eq!(
            limit.check(same_shard, 5 * SEC + SEC / 2),
            Err(Refused::Full)
        );
        assert_eq!(swept_at(key), Some(5 * SEC));
        assert!(limit.check(same_shard, 11 * SEC).is_ok());
    }

    #[test]
    fn the_global_limit_holds_across_threads() {
        let limit = std::sync::Arc::new(Global::new(Rate::per(1000, Duration::from_secs(1))));
        let admitted = std::sync::Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let limit = limit.clone();
                let admitted = admitted.clone();
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        if limit.check(0).is_ok() {
                            admitted.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(admitted.load(Ordering::Relaxed), 1000);
    }
}
