//! Whether the server refused the credential this device signs in with.
//!
//! A revoked API key or a changed password fails every request the same way,
//! and each caller logs it and moves on: the library stays empty and nothing
//! says why. Sync and the profile probe, the two requests every signed-in
//! device makes, report what they hear here, and the apps show it until a
//! request with the same credential succeeds. A different credential is a
//! fresh start, so signing in again clears it without being told.

use parking_lot::Mutex;

use super::client::{SubsonicAuth, SubsonicError};

/// The credentials refused, and not since accepted. One in practice: the
/// configured one.
static REFUSED: Mutex<Vec<SubsonicAuth>> = Mutex::new(Vec::new());

/// Subsonic's answers that mean the credential itself was refused: wrong
/// username or password (40), token authentication not supported (41),
/// invalid API key (44).
pub fn is_refusal(e: &SubsonicError) -> bool {
    matches!(
        e,
        SubsonicError::Api {
            code: 40 | 41 | 44,
            ..
        }
    )
}

/// What a request signed by `auth` came back with. A refusal is held; any
/// other answer from the server shows the credential works and clears it. An
/// error that never reached the server says nothing either way.
pub fn observe<T>(auth: &SubsonicAuth, result: &Result<T, SubsonicError>) {
    match result {
        Ok(_) => set(auth, false),
        Err(e) => observe_error(auth, e),
    }
}

/// [`observe`], for a caller holding only the error.
pub fn observe_error(auth: &SubsonicAuth, e: &SubsonicError) {
    if is_refusal(e) {
        set(auth, true);
    } else if matches!(e, SubsonicError::Api { .. }) {
        set(auth, false);
    }
}

fn set(auth: &SubsonicAuth, refused: bool) {
    let changed = {
        let mut held = REFUSED.lock();
        let was = held.contains(auth);
        if refused && !was {
            held.push(auth.clone());
        } else if !refused && was {
            held.retain(|a| a != auth);
        }
        was != refused
    };
    if changed {
        if refused {
            log::warn!(
                "remote: {} refused the credential for {}",
                auth.base_url,
                auth.username
            );
        }
        crate::remote::devices::touch();
    }
}

/// Whether the server refused `auth` when it was last used.
pub fn refused(auth: &SubsonicAuth) -> bool {
    REFUSED.lock().contains(auth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::client::Credential;

    fn auth(key: &str) -> SubsonicAuth {
        SubsonicAuth::with(
            "http://refusal.test",
            "mate",
            Credential::ApiKey(key.into()),
        )
    }

    fn api(code: i32) -> Result<(), SubsonicError> {
        Err(SubsonicError::Api {
            code,
            message: String::new(),
        })
    }

    #[test]
    fn a_refusal_is_held_until_the_same_credential_works() {
        let old = auth("held-old");
        observe(&old, &api(44));
        assert!(refused(&old));
        // Not found is an answer: the credential signed it.
        observe(&old, &api(70));
        assert!(!refused(&old));
        observe(&old, &api(40));
        assert!(refused(&old));
        observe(&old, &Ok(()));
        assert!(!refused(&old));
    }

    #[test]
    fn a_new_credential_is_not_refused_and_an_outage_says_nothing() {
        let old = auth("outage-old");
        observe(&old, &api(41));
        assert!(!refused(&auth("outage-new")));
        observe(&old, &Err::<(), _>(SubsonicError::BadResponse));
        assert!(refused(&old));
    }
}
