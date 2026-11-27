//! Live Activities: a device showing another's playback on its lock screen,
//! kept up to date by push as that device's state changes.

use super::*;

/// A Live Activity on a phone showing another device, and where to push its
/// updates.
pub(super) struct Activity {
    pub(super) username: String,
    /// The phone showing it.
    pub(super) watcher: String,
    /// The device it shows.
    pub(super) target: String,
    /// The relay's handle for the activity.
    pub(super) handle: String,
    /// What it was last sent, so only a change goes out: Apple budgets these.
    pub(super) sent: Option<koan_relay_client::ActivityState>,
}

impl Registry {
    /// Where to push a Live Activity's updates: `watcher` shows `target`.
    /// `None` ends it.
    pub fn set_activity(&self, username: &str, watcher: &str, activity: Option<(String, String)>) {
        let mut activities = self.activities.lock();
        activities.retain(|a| !(a.username == username && a.watcher == watcher));
        let Some((handle, target)) = activity else {
            return;
        };
        activities.push(Activity {
            username: username.to_string(),
            watcher: watcher.to_string(),
            target: target.clone(),
            handle,
            sent: None,
        });
        drop(activities);
        self.update_activities(username, &target);
    }

    /// Push `target`'s state to every Live Activity showing it, where it has
    /// changed in a way the activity shows.
    pub(super) fn update_activities(&self, username: &str, target: &str) {
        let Some(pusher) = crate::push::pusher() else {
            return;
        };
        // A device shared with `username` is its owner's to look up, and the
        // owner's reports reach those it is shared with too.
        let owner = self
            .shared_owner(username, target)
            .unwrap_or_else(|| username.to_string());
        let watchers: Vec<String> = self
            .grants
            .lock()
            .iter()
            .filter(|g| g.owner == owner && g.device == target)
            .map(|g| g.grantee.clone())
            .chain(std::iter::once(owner.clone()))
            .collect();
        // Every report lands here, and most devices are shown by no activity:
        // the state, and the art in it, is worked out only for one that is.
        if !self
            .activities
            .lock()
            .iter()
            .any(|a| a.target == target && watchers.contains(&a.username))
        {
            return;
        }
        let Some(info) = self
            .list(Some(&owner))
            .into_iter()
            .find(|c| c.device == target)
        else {
            return;
        };
        let state = crate::push::activity_state(&info);
        let mut due = Vec::new();
        for a in self.activities.lock().iter_mut() {
            if watchers.contains(&a.username)
                && a.target == target
                && a.sent
                    .as_ref()
                    .is_none_or(|s| crate::push::differs(&state, s))
            {
                a.sent = Some(state.clone());
                due.push((a.handle.clone(), a.watcher.clone(), a.username.clone()));
            }
        }
        if due.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            for (handle, watcher, username) in due {
                let push = crate::push::Push::Activity(state.clone());
                match pusher.send(&handle, &push) {
                    crate::push::Outcome::Sent => {}
                    crate::push::Outcome::Gone => {
                        log::info!("push: a Live Activity on {watcher} has ended");
                        registry().set_activity(&username, &watcher, None);
                    }
                    crate::push::Outcome::Failed(e) => {
                        log::warn!("push: Live Activity on {watcher}: {e}");
                    }
                }
            }
        });
    }
}
