//! Changes to accounts, for whatever outlives the request that
//! authenticated it.
//!
//! A request is authenticated once, but a socket or an event stream outlives
//! it and asks the database nothing afterwards. So every change that can
//! narrow what an account's connections hold (a new role or password, a
//! revoked key or app password, the account deleted) is announced here, by
//! the function in this crate that makes it, and a connection held under a
//! [`Lease`] ends when its account changes. The client reconnects and is
//! authenticated as things now stand.
//!
//! A change made in a transaction is announced again once it commits. The
//! first announcement comes as the row is written, and a connection it ended
//! could authenticate again before the commit, against the account as it was.

/// Changes to accounts, as a running count, and the count at each account's
/// latest change.
///
/// Revoking a session is not such a change: no socket rests on one, and
/// closing every socket on the account at each sign-out would only make them
/// all reconnect.
///
/// Only within this process: a change made by another, such as a CLI's,
/// reaches sockets when they next reconnect.
struct AccountChanges {
    count: tokio::sync::watch::Sender<u64>,
    latest: parking_lot::Mutex<std::collections::HashMap<i64, u64>>,
}

fn account_changes() -> &'static AccountChanges {
    static CHANGES: std::sync::OnceLock<AccountChanges> = std::sync::OnceLock::new();
    CHANGES.get_or_init(|| AccountChanges {
        count: tokio::sync::watch::Sender::new(0),
        latest: Default::default(),
    })
}

/// Announce a change to `user_id`'s account.
pub fn account_changed(user_id: i64) {
    let changes = account_changes();
    changes.count.send_modify(|count| {
        *count += 1;
        changes.latest.lock().insert(user_id, *count);
    });
}

/// Where the count of changes stands. Taken before a credential is checked,
/// so a change made while it is being checked still counts against it.
pub fn account_mark() -> u64 {
    *account_changes().count.borrow()
}

/// Resolves once any account has changed after `mark`, with the mark it now
/// stands at, to wait from next.
pub async fn any_account_changed_since(mark: u64) -> u64 {
    let mut count = account_changes().count.subscribe();
    loop {
        let now = *count.borrow_and_update();
        if now > mark {
            return now;
        }
        if count.changed().await.is_err() {
            return std::future::pending().await;
        }
    }
}

/// Resolves once `user_id`'s account has changed after `mark`.
pub async fn account_changed_since(user_id: i64, mark: u64) {
    let changes = account_changes();
    let mut count = changes.count.subscribe();
    loop {
        if changes
            .latest
            .lock()
            .get(&user_id)
            .is_some_and(|&at| at > mark)
        {
            return;
        }
        if count.changed().await.is_err() {
            return std::future::pending().await;
        }
    }
}

/// What a connection opened with a credential is held under: its account as
/// of `mark` ([`account_mark`]), and for a token, when it lapses.
#[derive(Debug, Clone, Copy)]
pub struct Lease {
    pub user_id: i64,
    pub mark: u64,
    /// Unix seconds.
    pub expires: Option<u64>,
}

impl Lease {
    /// Resolves when a connection held under this lease must close: its
    /// account changed, or its token lapsed. The client reconnects and
    /// authenticates again, so it holds no more than its credential now gives
    /// it.
    pub async fn ended(self) {
        let lapsed = async {
            match self.expires {
                Some(at) => {
                    let left = at.saturating_sub(crate::clock::unix_secs() as u64);
                    tokio::time::sleep(std::time::Duration::from_secs(left)).await;
                }
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = account_changed_since(self.user_id, self.mark) => {}
            _ = lapsed => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn a_lease_ends_when_its_account_changes_and_no_other() {
        let lease = Lease {
            user_id: 9001,
            mark: account_mark(),
            expires: None,
        };
        account_changed(9002);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), lease.ended())
                .await
                .is_err()
        );
        account_changed(9001);
        tokio::time::timeout(Duration::from_secs(1), lease.ended())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_lease_ends_when_its_token_lapses() {
        let lease = Lease {
            user_id: 9003,
            mark: account_mark(),
            expires: Some(crate::clock::unix_secs() as u64),
        };
        tokio::time::timeout(Duration::from_secs(1), lease.ended())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn any_change_after_a_mark_is_heard() {
        let mark = account_mark();
        account_changed(9004);
        let now = tokio::time::timeout(Duration::from_secs(1), any_account_changed_since(mark))
            .await
            .unwrap();
        assert!(now > mark);
    }
}
