//! Requests to one server in a few parallel lanes: the sync's song pages,
//! the playlists a sign-in pulls, the albums a listing left out.

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

/// Light requests in flight at once to one server (`getAlbum`,
/// `getPlaylist`): bound by round trips, not bandwidth. Measured against a
/// koan server over the internet, 128 `getAlbum`s took 4.3 s at 4 lanes,
/// 2.3 s at 8, 1.4 s at 16 and 0.8 s at 32, each request's median rising only
/// from 128 to 171 ms; playlists were quickest at 16. Sixteen takes most of
/// that while leaving a small server, a Navidrome on a Raspberry Pi, room.
pub const LANES: usize = 16;

/// `ask` for each of `items`, at most [`LANES`] at once, answers in the
/// order of `items`. Once `cancel` is set nothing more is asked, and an item
/// not asked answers `None`.
///
/// The lanes are a rayon pool of their own: the requests block on the
/// network, and the shared pool is for work that uses its cores.
pub fn in_lanes<T: Sync, R: Send>(
    items: &[T],
    cancel: &AtomicBool,
    ask: impl Fn(&T) -> R + Sync,
) -> Vec<Option<R>> {
    let ask = |item: &T| (!cancel.load(Ordering::Relaxed)).then(|| ask(item));
    let lanes = LANES.min(items.len()).max(1);
    match rayon::ThreadPoolBuilder::new()
        .num_threads(lanes)
        .thread_name(|i| format!("koan-lane-{i}"))
        .build()
    {
        Ok(pool) => pool.install(|| items.par_iter().map(ask).collect()),
        Err(e) => {
            log::warn!("no request lanes ({e}); asking one at a time");
            items.iter().map(ask).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use super::*;

    #[test]
    fn answers_keep_the_order_asked_and_no_more_than_the_lanes_run_at_once() {
        let running = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let items: Vec<usize> = (0..20).collect();
        let answers = in_lanes(&items, &AtomicBool::new(false), |&i| {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            // Later items answer sooner, so order kept is order restored.
            std::thread::sleep(Duration::from_millis(20 - i as u64));
            running.fetch_sub(1, Ordering::SeqCst);
            i * 10
        });
        let expected: Vec<Option<usize>> = (0..20).map(|i| Some(i * 10)).collect();
        assert_eq!(answers, expected);
        assert!(
            most.load(Ordering::SeqCst) <= LANES,
            "more than {LANES} at once"
        );
        assert!(most.load(Ordering::SeqCst) > 1, "never ran in parallel");
    }

    #[test]
    fn a_cancel_stops_what_is_not_yet_asked() {
        let cancel = AtomicBool::new(false);
        let asked = AtomicUsize::new(0);
        let items: Vec<usize> = (0..40).collect();
        let answers = in_lanes(&items, &cancel, |&i| {
            asked.fetch_add(1, Ordering::SeqCst);
            if i == 0 {
                cancel.store(true, Ordering::SeqCst);
            }
            std::thread::sleep(Duration::from_millis(5));
        });
        assert_eq!(answers.len(), 40);
        let asked = asked.load(Ordering::SeqCst);
        // Those already under way finish; nothing starts after.
        assert!(asked < 40, "all {asked} were asked after the cancel");
        assert_eq!(answers.iter().filter(|a| a.is_some()).count(), asked);
    }

    #[test]
    fn nothing_to_ask_asks_nothing() {
        let none: Vec<Option<()>> = in_lanes(&[] as &[u8], &AtomicBool::new(false), |_| ());
        assert!(none.is_empty());
    }
}
