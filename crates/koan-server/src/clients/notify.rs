//! Telling devices the server changed something they keep a copy of: a sync
//! down each link, and for a device that is away, one wake once the library
//! has gone quiet.

use super::*;

/// After `user` played or favourited something: re-evaluate their smart
/// playlists that read it, and have the devices that can see any that moved
/// pull them, so a "most played" list on an idle device does not wait for its
/// next read.
pub fn smart_activity(
    db: &koan_core::db::connection::Database,
    user: i64,
    fields: &[koan_core::smart::Field],
) {
    match koan_core::db::queries::smart::refresh_after_activity(&db.conn, user, fields) {
        Ok(moved) if !moved.is_empty() => changed_for(
            queries::private_owner(&db.conn, &moved)
                .ok()
                .flatten()
                .as_deref(),
        ),
        Ok(_) => {}
        Err(e) => log::warn!("smart playlists not refreshed after activity: {e}"),
    }
}

/// Have every device pull what the server just changed (a playlist edited,
/// albums added): at once where linked, on next link where not. Syncs waiting
/// for a device collapse into one, and so do the pushes that wake it: see
/// `Wakes`.
pub fn changed() {
    changed_for(None);
}

/// `changed`, for `username`'s devices alone, or everyone's for `None`.
pub(super) fn changed_for(username: Option<&str>) {
    let (_, queued) = registry().link_or_queue(username, &LinkCommand::Sync { full: false });
    if queued.is_empty() || crate::push::pusher().is_none() {
        return;
    }
    let mut wakes = WAKES.lock();
    wakes.add(queued.iter().map(Absent::key), std::time::Instant::now());
    if !wakes.timer {
        wakes.timer = true;
        std::thread::spawn(send_wakes);
    }
}

/// How long the library has to stay still before a suspended device is woken
/// to sync. A download of several albums scans after each one; iOS rations
/// background pushes, and the device needs waking once, at the end.
pub(super) const QUIET: std::time::Duration = std::time::Duration::from_secs(30);

/// However busy the library stays, a device waits no longer than this.
pub(super) const LONGEST_WAIT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

pub(super) static WAKES: Mutex<Wakes> = Mutex::new(Wakes {
    pending: Vec::new(),
    timer: false,
});

/// Background pushes held back until the library is quiet: at most one
/// pending per device.
pub(super) struct Wakes {
    /// `(device, username)`, when first asked for, when last asked for.
    pub(super) pending: Vec<((String, String), std::time::Instant, std::time::Instant)>,
    /// Whether a thread is waiting to send them.
    pub(super) timer: bool,
}

impl Wakes {
    pub(super) fn add(
        &mut self,
        devices: impl IntoIterator<Item = (String, String)>,
        now: std::time::Instant,
    ) {
        for key in devices {
            match self.pending.iter_mut().find(|(k, _, _)| *k == key) {
                Some((_, _, last)) => *last = now,
                None => self.pending.push((key, now, now)),
            }
        }
    }

    pub(super) fn due_at(
        first: std::time::Instant,
        last: std::time::Instant,
    ) -> std::time::Instant {
        (last + QUIET).min(first + LONGEST_WAIT)
    }

    /// The earliest a pending wake is due.
    pub(super) fn next(&self) -> Option<std::time::Instant> {
        self.pending
            .iter()
            .map(|(_, first, last)| Self::due_at(*first, *last))
            .min()
    }

    /// Take the wakes due by `now`.
    pub(super) fn take_due(&mut self, now: std::time::Instant) -> Vec<(String, String)> {
        let (due, waiting) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|(_, first, last)| Self::due_at(*first, *last) <= now);
        self.pending = waiting;
        due.into_iter().map(|(key, _, _)| key).collect()
    }
}

/// Send each wake once it is due, until none is pending. A device that has
/// linked meanwhile took its sync down the link and is not pushed.
pub(super) fn send_wakes() {
    loop {
        let (due, next) = {
            let mut wakes = WAKES.lock();
            let due = wakes.take_due(std::time::Instant::now());
            let next = wakes.next();
            if due.is_empty() && next.is_none() {
                wakes.timer = false;
                return;
            }
            (due, next)
        };
        let live = registry().live();
        let absent: Vec<(String, String)> = due.into_iter().filter(|d| !live.contains(d)).collect();
        if !absent.is_empty() {
            wake(&absent);
        }
        if let Some(next) = next {
            std::thread::sleep(next.saturating_duration_since(std::time::Instant::now()));
        }
    }
}

/// After a library scan or sync: if the library has changed since this was
/// last asked (`queries::library_stamp`: rows added, removed or rewritten),
/// tell every device. A scan that found nothing new, which is most of them,
/// sends nothing.
pub fn changed_if_library_moved(conn: &rusqlite::Connection) {
    static LAST: parking_lot::Mutex<Option<u64>> = parking_lot::Mutex::new(None);
    let Ok(now) = queries::library_stamp(conn).map(|stamp| stamp.rows) else {
        return;
    };
    let before = LAST.lock().replace(now);
    if before.is_some_and(|b| b != now) {
        changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_of_changes_wakes_each_device_once_when_it_goes_quiet() {
        let t0 = std::time::Instant::now();
        let s = std::time::Duration::from_secs;
        let phone = || ("dev-1".to_string(), "j".to_string());
        let ipad = || ("dev-2".to_string(), "j".to_string());
        let mut wakes = Wakes {
            pending: Vec::new(),
            timer: false,
        };

        wakes.add([phone()], t0);
        wakes.add([phone(), ipad()], t0 + s(10));
        wakes.add([phone()], t0 + s(20));
        assert_eq!(wakes.pending.len(), 2);

        // Quiet is measured from each device's last change.
        assert!(wakes.take_due(t0 + s(39)).is_empty());
        assert_eq!(wakes.take_due(t0 + s(40)), [ipad()]);
        assert_eq!(wakes.next(), Some(t0 + s(50)));
        assert_eq!(wakes.take_due(t0 + s(50)), [phone()]);
        assert_eq!(wakes.next(), None);
    }

    #[test]
    fn a_library_that_never_goes_quiet_still_wakes_devices() {
        let t0 = std::time::Instant::now();
        let phone = || ("dev-1".to_string(), "j".to_string());
        let mut wakes = Wakes {
            pending: Vec::new(),
            timer: false,
        };
        let mut sent = 0;
        for i in 0..40 {
            let now = t0 + std::time::Duration::from_secs(i * 10);
            sent += wakes.take_due(now).len();
            wakes.add([phone()], now);
        }
        // Six and a half minutes of changes every ten seconds: one push at
        // the five-minute mark, and one pending.
        assert_eq!(sent, 1);
        assert_eq!(wakes.pending.len(), 1);
    }
}
