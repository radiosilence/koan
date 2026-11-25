//! API keys (OpenSubsonic `apiKeyAuthentication`), from `koan-auth`, and the
//! device a key signed in, which kōan's link binds it to.

use rusqlite::{Connection, OptionalExtension, params};

pub use koan_auth::api_keys::{
    ApiKeyRow, authenticate_api_key, create_api_key, id_of, list_api_keys, replace_api_key,
    revoke_api_key, revoke_api_key_value, revoke_user_api_keys,
};

use crate::auth;

/// A device's public key, as the account's devices are sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceKey {
    pub device: String,
    /// Base64 of the device's 32-byte Ed25519 public key.
    pub key: String,
    /// The account it belongs to, for a device shared with `username`;
    /// `None` for its own.
    pub owner: Option<String>,
}

/// Record that the API key `raw_key` signed in `device`, which proves itself
/// with `public_key`. Whether it was kept.
///
/// A key row is bound to the first device it records, so a thief holding one
/// device's key cannot link as another device's id and have its own public
/// key published in that device's place. A key not yet bound to a device is
/// one just made by signing in, and it takes the device over: the account's
/// other keys for that device are revoked, since signing in again by any
/// route (password, invite, pairing) leaves the device holding only the new
/// one. A new keypair on the same key replaces the old public key.
pub fn set_device_key(
    conn: &Connection,
    raw_key: &str,
    device: &str,
    public_key: &str,
) -> rusqlite::Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let Some((id, user_id, bound)) = tx
        .query_row(
            "SELECT id, user_id, device FROM api_keys WHERE key_hash = ?1",
            [auth::sha256_hex(raw_key)],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(false);
    };
    if bound.as_deref().is_some_and(|b| b != device) {
        return Ok(false);
    }
    let retired = tx.execute(
        "DELETE FROM api_keys WHERE user_id = ?1 AND device = ?2 AND id != ?3",
        params![user_id, device, id],
    )?;
    tx.execute(
        "UPDATE api_keys SET device = ?1, device_key = ?2 WHERE id = ?3",
        params![device, public_key, id],
    )?;
    tx.commit()?;
    if retired > 0 {
        auth::account_changed(user_id);
    }
    Ok(true)
}

/// The public keys `username`'s devices prove themselves with, and those of
/// the devices other accounts share with `username`: the newest key each
/// device registered under a key still standing.
pub fn device_keys(conn: &Connection, username: &str) -> rusqlite::Result<Vec<DeviceKey>> {
    let mut stmt = conn.prepare_cached(
        "SELECT k.device, k.device_key, u.username
           FROM api_keys k JOIN users u ON u.id = k.user_id
          WHERE k.device IS NOT NULL AND k.device_key IS NOT NULL
            AND (u.username = ?1
                 OR EXISTS (SELECT 1 FROM link_grants g
                             WHERE g.grantee = ?1 AND g.owner = u.username
                               AND g.device = k.device))
          ORDER BY k.created_at, k.id",
    )?;
    let rows = stmt.query_map([username], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    // Oldest first, so a later key for the same device replaces it.
    let mut keys: Vec<DeviceKey> = Vec::new();
    for row in rows {
        let (device, key, owner) = row?;
        let owner = (owner != username).then_some(owner);
        keys.retain(|k| !(k.device == device && k.owner == owner));
        keys.push(DeviceKey { device, key, owner });
    }
    Ok(keys)
}

#[cfg(test)]
mod device_key_tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        for name in ["jo", "kim"] {
            crate::db::queries::auth::create_user(&conn, name, "pw", auth::Role::User).unwrap();
        }
        conn
    }

    fn user(conn: &Connection, name: &str) -> i64 {
        crate::db::queries::auth::get_user_by_username(conn, name)
            .unwrap()
            .unwrap()
            .id
    }

    #[test]
    fn a_key_records_its_device_and_is_published_until_revoked() {
        let conn = db();
        let jo = user(&conn, "jo");
        let (_, phone) = create_api_key(&conn, jo, "phone").unwrap();
        let (mac_id, mac) = create_api_key(&conn, jo, "mac").unwrap();
        assert!(set_device_key(&conn, &phone, "dev-phone", "PHONE1").unwrap());
        assert!(set_device_key(&conn, &mac, "dev-mac", "MAC").unwrap());
        assert!(!set_device_key(&conn, "not a key", "dev-x", "X").unwrap());
        // Signing in again on the phone: its key replaced, a new keypair.
        let (_, phone2) = replace_api_key(&conn, jo, "phone").unwrap();
        assert!(set_device_key(&conn, &phone2, "dev-phone", "PHONE2").unwrap());

        let keys = device_keys(&conn, "jo").unwrap();
        let of = |d: &str| keys.iter().find(|k| k.device == d).map(|k| k.key.as_str());
        assert_eq!(of("dev-phone"), Some("PHONE2"));
        assert_eq!(of("dev-mac"), Some("MAC"));
        assert!(keys.iter().all(|k| k.owner.is_none()));
        assert!(device_keys(&conn, "kim").unwrap().is_empty());

        revoke_api_key(&conn, mac_id, None).unwrap();
        let keys = device_keys(&conn, "jo").unwrap();
        assert_eq!(keys.len(), 1, "the Mac's key went with its API key");
    }

    /// The phone's key, stolen, cannot take the Mac's place: a key row
    /// belongs to the device it first recorded.
    #[test]
    fn a_key_cannot_claim_another_devices_id() {
        let conn = db();
        let jo = user(&conn, "jo");
        let (_, phone) = create_api_key(&conn, jo, "phone").unwrap();
        let (_, mac) = create_api_key(&conn, jo, "mac").unwrap();
        assert!(set_device_key(&conn, &mac, "dev-mac", "MAC").unwrap());
        assert!(set_device_key(&conn, &phone, "dev-phone", "PHONE").unwrap());
        assert!(!set_device_key(&conn, &phone, "dev-mac", "THIEF").unwrap());
        assert!(!set_device_key(&conn, &phone, "dev-new", "THIEF").unwrap());
        // Its own id, with a new keypair, is still its to change.
        assert!(set_device_key(&conn, &phone, "dev-phone", "PHONE2").unwrap());
        let keys = device_keys(&conn, "jo").unwrap();
        let of = |d: &str| keys.iter().find(|k| k.device == d).map(|k| k.key.as_str());
        assert_eq!(of("dev-mac"), Some("MAC"));
        assert_eq!(of("dev-phone"), Some("PHONE2"));
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn a_shared_device_is_published_to_its_grantee_with_its_owner() {
        let conn = db();
        let jo = user(&conn, "jo");
        let (_, phone) = create_api_key(&conn, jo, "phone").unwrap();
        let (_, mac) = create_api_key(&conn, jo, "mac").unwrap();
        set_device_key(&conn, &phone, "dev-phone", "PHONE").unwrap();
        set_device_key(&conn, &mac, "dev-mac", "MAC").unwrap();
        conn.execute(
            "INSERT INTO link_grants (device, owner, grantee, created_at)
             VALUES ('dev-phone', 'jo', 'kim', 0)",
            [],
        )
        .unwrap();
        assert_eq!(
            device_keys(&conn, "kim").unwrap(),
            vec![DeviceKey {
                device: "dev-phone".into(),
                key: "PHONE".into(),
                owner: Some("jo".into()),
            }]
        );
    }

    /// Signed out and in again by a route that mints a key beside the old
    /// one: the new key takes the device, and the old key goes.
    #[test]
    fn a_new_key_takes_its_device_from_the_old_one() {
        let conn = db();
        let jo = user(&conn, "jo");
        let kim = user(&conn, "kim");
        let (_, old) = create_api_key(&conn, jo, "phone").unwrap();
        let (_, theirs) = create_api_key(&conn, kim, "phone").unwrap();
        assert!(set_device_key(&conn, &old, "dev-phone", "PHONE1").unwrap());
        assert!(set_device_key(&conn, &theirs, "dev-phone", "KIM").unwrap());

        let (_, new) = create_api_key(&conn, jo, "iPhone").unwrap();
        assert!(set_device_key(&conn, &new, "dev-phone", "PHONE2").unwrap());
        assert!(
            id_of(&conn, &old).unwrap().is_none(),
            "the old key is retired"
        );
        assert!(
            id_of(&conn, &theirs).unwrap().is_some(),
            "another account's is not"
        );
        let keys = device_keys(&conn, "jo").unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, "PHONE2");
    }
}
