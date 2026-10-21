//! Waking an Apple TV and starting koan on it, over the Companion protocol
//! that Control Center's remote and the Apple TV Remote app use
//! (`_companion-link._tcp`).
//!
//! Pairing is once per account: the TV shows a PIN, a device of the account
//! on its network types it in (`begin_pairing`, then `Pairing::finish`), and
//! the resulting `Credential` is kept by the server for every device of the
//! account. Waking (`wake`) is then an authenticated session that sends the
//! wake button, which turns the box and its display on, and launches koan,
//! which links to its server and is sent the music as any device awake is.
//! No push is involved, so it works whoever signed the app.
//!
//! The wire format follows pyatv's implementation: 4-byte frame headers,
//! HomeKit pair-setup (SRP-6a) and pair-verify (X25519, Ed25519) carried in
//! OPACK dictionaries as TLV8, then ChaCha20-Poly1305 frames of OPACK
//! requests. The TV closes a connection 30 seconds after it opens unless
//! pairing finishes, so a PIN is asked for only once the TV is showing it.

pub mod opack;
mod srp;
mod tlv8;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use opack::Value;
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

/// The Bonjour service a TV answers the protocol on.
pub const SERVICE: &str = "_companion-link._tcp";
/// What the TV launches.
pub const BUNDLE_ID: &str = "cc.blit.koan";

/// What pairing leaves: enough to authenticate to the TV again, and to know
/// it is the same TV. Held by the server per account.
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    /// The TV's pairing identifier, which its announcement carries as
    /// `rpMRtID`: how the TV is found again.
    pub tv_id: String,
    /// The TV's long-term Ed25519 public key, base64.
    pub tv_key: String,
    /// This side's pairing identifier.
    pub id: String,
    /// This side's long-term Ed25519 seed, base64.
    pub seed: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("couldn't reach the Apple TV: {0}")]
    Io(#[from] std::io::Error),
    #[error("that PIN isn't the one on the TV")]
    WrongPin,
    #[error("the Apple TV is busy pairing with something else; try again in a moment")]
    Busy,
    #[error("the Apple TV refused: {0}")]
    Refused(String),
    #[error("the Apple TV's answer made no sense: {0}")]
    Protocol(&'static str),
    #[error("that isn't the Apple TV this was paired with")]
    WrongTv,
    #[error("the pairing is no longer valid; pair the Apple TV again")]
    Stale,
}

impl From<opack::Malformed> for Error {
    fn from(_: opack::Malformed) -> Self {
        Error::Protocol("malformed message")
    }
}

/// How long one answer is waited for.
const ANSWER: Duration = Duration::from_secs(3);
/// How long connecting is given.
const CONNECT: Duration = Duration::from_secs(4);
/// The longest `wake` can take: connecting, the two pair-verify round trips
/// and the seven requests, each waited on for `ANSWER`, and the TV coming up.
pub const LONGEST_WAKE: Duration =
    Duration::from_secs(CONNECT.as_secs() + 9 * ANSWER.as_secs() + AWAKE_WITHIN.as_secs());
/// The largest frame read. A TV's answers are a few kilobytes; a length past
/// this is from something that is not one.
const MAX_FRAME: usize = 1 << 20;

#[derive(Clone, Copy)]
#[repr(u8)]
enum Frame {
    PairSetupStart = 3,
    PairSetupNext = 4,
    PairVerifyStart = 5,
    PairVerifyNext = 6,
    Encrypted = 8,
}

/// An event, a request and its response, as `_t` says.
const EVENT: i64 = 1;
const REQUEST: i64 = 2;
const RESPONSE: i64 = 3;

struct Cipher {
    out: LessSafeKey,
    input: LessSafeKey,
    sent: u64,
    received: u64,
}

fn counter_nonce(n: u64) -> Nonce {
    let mut b = [0u8; 12];
    b[..8].copy_from_slice(&n.to_le_bytes());
    Nonce::assume_unique_for_key(b)
}

/// The nonce pairing messages are sealed with: four zeros, then the label.
fn label_nonce(label: &[u8; 8]) -> Nonce {
    let mut b = [0u8; 12];
    b[4..].copy_from_slice(label);
    Nonce::assume_unique_for_key(b)
}

fn key(bytes: &[u8]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, bytes).expect("a 32-byte key"))
}

fn seal(key: &LessSafeKey, nonce: Nonce, aad: &[u8], data: &[u8]) -> Vec<u8> {
    let mut buf = data.to_vec();
    key.seal_in_place_append_tag(nonce, Aad::from(aad), &mut buf)
        .expect("sealing a frame");
    buf
}

fn open(key: &LessSafeKey, nonce: Nonce, aad: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut buf = data.to_vec();
    let len = key
        .open_in_place(nonce, Aad::from(aad), &mut buf)
        .map_err(|_| Error::Protocol("a frame failed to decrypt"))?
        .len();
    buf.truncate(len);
    Ok(buf)
}

fn hkdf(salt: &str, info: &str, secret: &[u8]) -> [u8; 32] {
    struct Len;
    impl ring::hkdf::KeyType for Len {
        fn len(&self) -> usize {
            32
        }
    }
    let mut out = [0u8; 32];
    ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA512, salt.as_bytes())
        .extract(secret)
        .expand(&[info.as_bytes()], Len)
        .expect("32 bytes from HKDF-SHA512")
        .fill(&mut out)
        .expect("32 bytes from HKDF-SHA512");
    out
}

fn random<const N: usize>() -> [u8; N] {
    use ring::rand::SecureRandom;
    let mut out = [0u8; N];
    ring::rand::SystemRandom::new()
        .fill(&mut out)
        .expect("the system's random source");
    out
}

/// One connection to a TV.
struct Conn {
    stream: TcpStream,
    cipher: Option<Cipher>,
    xid: i64,
    /// The TV's system status as it last said, once subscribed: asleep,
    /// screensaver or awake (`AWAKE`).
    status: Option<i64>,
}

/// The system status of a TV that is on, out of its screensaver.
const AWAKE: i64 = 3;
/// How long a TV waking is given to say it is awake.
const AWAKE_WITHIN: Duration = Duration::from_secs(8);
/// After a TV that says nothing of its status is woken, how long before
/// koan is launched: launched while the box is still coming up, it is sent
/// to the background as the home screen arrives, and suspended there.
const SETTLE: Duration = Duration::from_secs(3);

impl Conn {
    fn open(addr: SocketAddr) -> Result<Self, Error> {
        let stream = TcpStream::connect_timeout(&addr, CONNECT)?;
        stream.set_read_timeout(Some(ANSWER))?;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            cipher: None,
            xid: i64::from(u16::from_le_bytes(random())),
            status: None,
        })
    }

    fn send(&mut self, frame: Frame, payload: &[u8]) -> Result<(), Error> {
        let sealed = self.cipher.is_some() && !payload.is_empty();
        let len = payload.len() + if sealed { 16 } else { 0 };
        let len = u32::try_from(len)
            .ok()
            .filter(|l| *l < 1 << 24)
            .ok_or(Error::Protocol("a frame too long to send"))?;
        let mut header = [frame as u8, 0, 0, 0];
        header[1..].copy_from_slice(&len.to_be_bytes()[1..]);
        let body = match &mut self.cipher {
            Some(c) if sealed => {
                let n = counter_nonce(c.sent);
                c.sent += 1;
                seal(&c.out, n, &header, payload)
            }
            _ => payload.to_vec(),
        };
        self.stream.write_all(&[&header[..], &body].concat())?;
        Ok(())
    }

    fn receive(&mut self) -> Result<(u8, Vec<u8>), Error> {
        let mut header = [0u8; 4];
        self.stream.read_exact(&mut header)?;
        let len = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        if len > MAX_FRAME {
            return Err(Error::Protocol("a frame too long to be a TV's"));
        }
        let mut body = vec![0u8; len];
        self.stream.read_exact(&mut body)?;
        if let Some(c) = &mut self.cipher
            && !body.is_empty()
        {
            let n = counter_nonce(c.received);
            c.received += 1;
            body = open(&c.input, n, &header, &body)?;
        }
        Ok((header[0], body))
    }

    /// Send a pairing message and read the TLV8 the answer carries.
    fn pair(
        &mut self,
        frame: Frame,
        tlv: &[(u8, &[u8])],
        extra: &[(&str, Value)],
    ) -> Result<Vec<(u8, Vec<u8>)>, Error> {
        let mut fields = vec![(Value::from("_pd"), Value::Bytes(tlv8::write(tlv)))];
        fields.extend(extra.iter().map(|(k, v)| (Value::from(*k), v.clone())));
        fields.push((Value::from("_x"), Value::Int(self.xid)));
        self.xid += 1;
        self.send(frame, &opack::pack(&Value::Dict(fields)))?;
        let answer = loop {
            let (kind, body) = self.receive()?;
            if kind == Frame::PairSetupNext as u8 || kind == Frame::PairVerifyNext as u8 {
                break opack::unpack(&body)?;
            }
        };
        let data = answer
            .get("_pd")
            .and_then(Value::as_bytes)
            .ok_or(Error::Protocol("no pairing data"))?;
        let items = tlv8::read(data).ok_or(Error::Protocol("malformed pairing data"))?;
        if let Some(code) = tlv8::find(&items, tlv8::ERROR) {
            return Err(match code.first() {
                Some(2) => Error::WrongPin,
                Some(3 | 7) => Error::Busy,
                _ if tlv8::find(&items, tlv8::BACK_OFF).is_some() => Error::Busy,
                Some(c) => Error::Refused(format!("pairing error {c}")),
                None => Error::Refused("pairing error".into()),
            });
        }
        Ok(items)
    }

    /// Send a request and wait for its answer, passing over events.
    fn request(&mut self, name: &str, content: Value) -> Result<Value, Error> {
        let xid = self.xid;
        self.xid += 1;
        let msg = Value::dict([
            ("_i", name.into()),
            ("_t", REQUEST.into()),
            ("_c", content),
            ("_x", xid.into()),
        ]);
        self.send(Frame::Encrypted, &opack::pack(&msg))?;
        loop {
            let Some(answer) = self.message()? else {
                continue;
            };
            if answer.get("_t").and_then(Value::as_int) != Some(RESPONSE)
                || answer.get("_x").and_then(Value::as_int) != Some(xid)
            {
                continue;
            }
            if let Some(why) = answer.get("_em").and_then(Value::as_str) {
                return Err(Error::Refused(why.to_string()));
            }
            return Ok(answer);
        }
    }

    /// Send an event, which has no answer.
    fn event(&mut self, name: &str, content: Value) -> Result<(), Error> {
        let xid = self.xid;
        self.xid += 1;
        let msg = Value::dict([
            ("_i", name.into()),
            ("_t", EVENT.into()),
            ("_c", content),
            ("_x", xid.into()),
        ]);
        self.send(Frame::Encrypted, &opack::pack(&msg))
    }

    /// The next message, noting a system status event on the way. `None`
    /// for a frame that is not one.
    fn message(&mut self) -> Result<Option<Value>, Error> {
        let (kind, body) = self.receive()?;
        if kind != Frame::Encrypted as u8 || body.is_empty() {
            return Ok(None);
        }
        let msg = opack::unpack(&body)?;
        let status = matches!(
            msg.get("_i").and_then(Value::as_str),
            Some("SystemStatus" | "TVSystemStatus")
        );
        if status && let Some(state) = msg.get("_c").and_then(|c| c.get("state")) {
            self.status = state.as_int();
            log::info!("companion: system status {:?}", self.status);
        }
        Ok(Some(msg))
    }

    /// Wait up to `within` for the TV to say it is awake. Whether it did.
    fn await_awake(&mut self, within: Duration) -> Result<bool, Error> {
        let until = std::time::Instant::now() + within;
        while self.status != Some(AWAKE) {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Ok(false);
            }
            self.stream.set_read_timeout(Some(left))?;
            match self.message() {
                Ok(_) => {}
                Err(Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(false);
                }
                Err(e) => return Err(e),
            }
        }
        self.stream.set_read_timeout(Some(ANSWER))?;
        Ok(true)
    }
}

/// A pairing under way: the TV is showing its PIN.
pub struct Pairing {
    conn: Conn,
    salt: Vec<u8>,
    server_public: Vec<u8>,
}

/// Ask the TV at `addr` to pair, which puts a PIN on its screen. Finish
/// within half a minute: the TV hangs up after that.
pub fn begin_pairing(addr: SocketAddr) -> Result<Pairing, Error> {
    let mut conn = Conn::open(addr)?;
    // The PIN is typed by a person: give them the TV's whole half minute.
    conn.stream
        .set_read_timeout(Some(Duration::from_secs(30)))?;
    let items = conn.pair(
        Frame::PairSetupStart,
        &[(tlv8::METHOD, &[0]), (tlv8::STATE, &[1])],
        &[("_pwTy", Value::Int(1))],
    )?;
    let salt = tlv8::find(&items, tlv8::SALT).ok_or(Error::Protocol("no salt"))?;
    let public = tlv8::find(&items, tlv8::PUBLIC_KEY).ok_or(Error::Protocol("no public key"))?;
    Ok(Pairing {
        salt: salt.to_vec(),
        server_public: public.to_vec(),
        conn,
    })
}

impl Pairing {
    /// Finish with the PIN the TV shows. `name` is what the TV lists the
    /// pairing as, under Remotes and Devices.
    pub fn finish(mut self, pin: &str, name: &str) -> Result<Credential, Error> {
        let pin: String = pin.chars().filter(char::is_ascii_digit).collect();
        let exchange = srp::Exchange::new(&pin, &self.salt, &self.server_public, &random::<32>())
            .ok_or(Error::Protocol("an invalid public key"))?;
        let items = self.conn.pair(
            Frame::PairSetupNext,
            &[
                (tlv8::STATE, &[3]),
                (tlv8::PUBLIC_KEY, &exchange.public),
                (tlv8::PROOF, &exchange.proof),
            ],
            &[("_pwTy", Value::Int(1))],
        )?;
        let proof = tlv8::find(&items, tlv8::PROOF).ok_or(Error::Protocol("no proof"))?;
        if !exchange.verify(proof) {
            return Err(Error::WrongPin);
        }

        let seed = random::<32>();
        let keys = Ed25519KeyPair::from_seed_unchecked(&seed).expect("a 32-byte seed");
        let own_key = keys.public_key().as_ref().to_vec();
        let id = uuid::Uuid::now_v7().to_string().to_uppercase();
        let sign_x = hkdf(
            "Pair-Setup-Controller-Sign-Salt",
            "Pair-Setup-Controller-Sign-Info",
            &exchange.key,
        );
        let signature = keys.sign(&[&sign_x[..], id.as_bytes(), &own_key].concat());
        let named = opack::pack(&Value::dict([("name", name.into())]));
        let inner = tlv8::write(&[
            (tlv8::IDENTIFIER, id.as_bytes()),
            (tlv8::PUBLIC_KEY, &own_key),
            (tlv8::SIGNATURE, signature.as_ref()),
            (tlv8::NAME, &named),
        ]);
        let session = key(&hkdf(
            "Pair-Setup-Encrypt-Salt",
            "Pair-Setup-Encrypt-Info",
            &exchange.key,
        ));
        let sealed = seal(&session, label_nonce(b"PS-Msg05"), &[], &inner);
        let items = self.conn.pair(
            Frame::PairSetupNext,
            &[(tlv8::STATE, &[5]), (tlv8::ENCRYPTED_DATA, &sealed)],
            &[("_pwTy", Value::Int(1))],
        )?;
        let sealed =
            tlv8::find(&items, tlv8::ENCRYPTED_DATA).ok_or(Error::Protocol("no device info"))?;
        let inner = open(&session, label_nonce(b"PS-Msg06"), &[], sealed)?;
        let items = tlv8::read(&inner).ok_or(Error::Protocol("malformed device info"))?;
        let tv_id = tlv8::find(&items, tlv8::IDENTIFIER).ok_or(Error::Protocol("no TV id"))?;
        let tv_key = tlv8::find(&items, tlv8::PUBLIC_KEY).ok_or(Error::Protocol("no TV key"))?;
        let tv_signature =
            tlv8::find(&items, tlv8::SIGNATURE).ok_or(Error::Protocol("no TV signature"))?;
        let accessory_x = hkdf(
            "Pair-Setup-Accessory-Sign-Salt",
            "Pair-Setup-Accessory-Sign-Info",
            &exchange.key,
        );
        UnparsedPublicKey::new(&ED25519, tv_key)
            .verify(&[&accessory_x[..], tv_id, tv_key].concat(), tv_signature)
            .map_err(|_| Error::Protocol("the TV's signature does not verify"))?;
        Ok(Credential {
            tv_id: String::from_utf8_lossy(tv_id).into_owned(),
            tv_key: B64.encode(tv_key),
            id,
            seed: B64.encode(seed),
        })
    }
}

/// Open an authenticated, encrypted session with the TV `credential` names.
fn verify(addr: SocketAddr, credential: &Credential) -> Result<Conn, Error> {
    let tv_key = B64.decode(&credential.tv_key).map_err(|_| Error::Stale)?;
    let seed = B64.decode(&credential.seed).map_err(|_| Error::Stale)?;
    let keys = Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| Error::Stale)?;
    let rng = ring::rand::SystemRandom::new();
    let ephemeral = ring::agreement::EphemeralPrivateKey::generate(&ring::agreement::X25519, &rng)
        .map_err(|_| Error::Protocol("no key agreement"))?;
    let own_public = ephemeral
        .compute_public_key()
        .map_err(|_| Error::Protocol("no key agreement"))?
        .as_ref()
        .to_vec();

    let mut conn = Conn::open(addr)?;
    let items = conn.pair(
        Frame::PairVerifyStart,
        &[(tlv8::STATE, &[1]), (tlv8::PUBLIC_KEY, &own_public)],
        &[("_auTy", Value::Int(4))],
    )?;
    let tv_public = tlv8::find(&items, tlv8::PUBLIC_KEY)
        .ok_or(Error::Protocol("no public key"))?
        .to_vec();
    let sealed = tlv8::find(&items, tlv8::ENCRYPTED_DATA).ok_or(Error::Protocol("no proof"))?;
    let shared = ring::agreement::agree_ephemeral(
        ephemeral,
        &ring::agreement::UnparsedPublicKey::new(&ring::agreement::X25519, &tv_public),
        |s| s.to_vec(),
    )
    .map_err(|_| Error::Protocol("key agreement failed"))?;
    let session = key(&hkdf(
        "Pair-Verify-Encrypt-Salt",
        "Pair-Verify-Encrypt-Info",
        &shared,
    ));
    // Sealed with this session's key exchange, not the pairing: a failure
    // here is the exchange's.
    let inner = open(&session, label_nonce(b"PV-Msg02"), &[], sealed)?;
    let items = tlv8::read(&inner).ok_or(Error::Protocol("malformed proof"))?;
    let tv_id = tlv8::find(&items, tlv8::IDENTIFIER).ok_or(Error::Protocol("no TV id"))?;
    let tv_signature =
        tlv8::find(&items, tlv8::SIGNATURE).ok_or(Error::Protocol("no signature"))?;
    if tv_id != credential.tv_id.as_bytes() {
        return Err(Error::WrongTv);
    }
    UnparsedPublicKey::new(&ED25519, &tv_key)
        .verify(&[&tv_public[..], tv_id, &own_public].concat(), tv_signature)
        .map_err(|_| Error::WrongTv)?;

    let signature = keys.sign(&[&own_public[..], credential.id.as_bytes(), &tv_public].concat());
    let inner = tlv8::write(&[
        (tlv8::IDENTIFIER, credential.id.as_bytes()),
        (tlv8::SIGNATURE, signature.as_ref()),
    ]);
    let sealed = seal(&session, label_nonce(b"PV-Msg03"), &[], &inner);
    conn.pair(
        Frame::PairVerifyNext,
        &[(tlv8::STATE, &[3]), (tlv8::ENCRYPTED_DATA, &sealed)],
        &[],
    )
    .map_err(|e| match e {
        // The TV no longer knows this pairing: removed under Remotes and
        // Devices, or the TV reset.
        Error::WrongPin | Error::Refused(_) => Error::Stale,
        e => e,
    })?;
    conn.cipher = Some(Cipher {
        out: key(&hkdf("", "ClientEncrypt-main", &shared)),
        input: key(&hkdf("", "ServerEncrypt-main", &shared)),
        sent: 0,
        received: 0,
    });
    Ok(conn)
}

/// Wake the TV at `addr` and launch koan on it. `name` is this device's,
/// for the session's introduction.
pub fn wake(addr: SocketAddr, credential: &Credential, name: &str) -> Result<(), Error> {
    let mut conn = verify(addr, credential)?;
    conn.request(
        "_systemInfo",
        Value::dict([
            ("_bf", 0.into()),
            ("_cf", 512.into()),
            ("_clFl", 128.into()),
            (
                "_i",
                Value::Str(
                    credential
                        .id
                        .to_lowercase()
                        .chars()
                        .filter(char::is_ascii_hexdigit)
                        .take(12)
                        .collect(),
                ),
            ),
            ("_idsID", Value::Bytes(credential.id.as_bytes().to_vec())),
            ("_pubID", credential.id.as_str().into()),
            ("_sf", 256.into()),
            ("_sv", "170.18".into()),
            ("model", "kōan".into()),
            ("name", name.into()),
        ]),
    )?;
    conn.request(
        "_sessionStart",
        Value::dict([
            ("_srvT", "com.apple.tvremoteservices".into()),
            ("_sid", i64::from(u32::from_le_bytes(random())).into()),
        ]),
    )?;
    // What the TV says of its status, asleep or awake: tvOS answers
    // `FetchAttentionState` only within a remote session, and some releases
    // not at all, so the events are what is waited on.
    let _ = conn.request(
        "TVRCSessionStart",
        Value::dict([("ProtocolVersionKey", "1.2".into())]),
    );
    conn.event(
        "_interest",
        Value::dict([(
            "_regEvents",
            Value::Array(vec!["SystemStatus".into(), "TVSystemStatus".into()]),
        )]),
    )?;
    if let Ok(answer) = conn.request("FetchAttentionState", Value::dict([]))
        && let Some(state) = answer.get("_c").and_then(|c| c.get("state"))
    {
        conn.status = state.as_int();
    }
    let asleep = conn.status != Some(AWAKE);
    if asleep {
        // The wake button, released: what the Remote app sends.
        conn.request(
            "_hidC",
            Value::dict([("_hBtS", 2.into()), ("_hidC", 13.into())]),
        )?;
        // Launched before the TV is up, koan is sent to the background when
        // the home screen arrives, and suspended there.
        let known = conn.status.is_some();
        if !conn.await_awake(AWAKE_WITHIN)? && !known {
            std::thread::sleep(SETTLE);
        }
    }
    log::info!("companion: launching (was asleep: {asleep})");
    conn.request("_launchApp", Value::dict([("_bundleID", BUNDLE_ID.into())]))?;
    Ok(())
}

/// An Apple TV announcing the protocol on this network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tv {
    /// The name set on the TV.
    pub name: String,
    /// The host it announces, as `Living-Room.local.`.
    pub host: String,
    pub addr: SocketAddr,
    /// Its pairing identifier, `rpMRtID`.
    pub id: Option<String>,
}

/// The Apple TVs on this network, looking for `within`. Other devices
/// announce the service too (Macs, HomePods); only Apple TVs are kept.
#[cfg(target_vendor = "apple")]
pub fn browse(within: Duration) -> Vec<Tv> {
    use std::net::ToSocketAddrs;
    crate::remote::nearby::bonjour::instances(SERVICE, within)
        .into_iter()
        .filter(|i| i.txt("rpMd").is_some_and(|m| m.starts_with("AppleTV")))
        .filter_map(|i| {
            let addr = (i.host.trim_end_matches('.'), i.port)
                .to_socket_addrs()
                .ok()?
                .min_by_key(|a| !a.is_ipv4())?;
            Some(Tv {
                id: i.txt("rpMRtID"),
                name: i.name,
                host: i.host,
                addr,
            })
        })
        .collect()
}

#[cfg(not(target_vendor = "apple"))]
pub fn browse(_within: Duration) -> Vec<Tv> {
    Vec::new()
}

/// The host of an address as dialled, `host:port`.
pub fn host_of(addr: &str) -> &str {
    addr.rsplit_once(':').map_or(addr, |(h, _)| h)
}

fn same_host(a: &str, b: &str) -> bool {
    let a = a.trim_end_matches('.');
    let b = b.trim_end_matches('.');
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

/// Which of `tvs` is the koan device met at `host` (a name or an address)
/// and named `name`: the one on that host, else the only one of that name,
/// else the only one. Two that nothing tells apart are not guessed between.
pub fn pick<'a>(tvs: &'a [Tv], host: &str, name: &str) -> Option<&'a Tv> {
    let ip = host
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .ok();
    if let Some(tv) = tvs
        .iter()
        .find(|t| same_host(&t.host, host) || Some(t.addr.ip()) == ip)
    {
        return Some(tv);
    }
    let named: Vec<&Tv> = tvs
        .iter()
        .filter(|t| t.name.trim().eq_ignore_ascii_case(name.trim()))
        .collect();
    match (named.as_slice(), tvs) {
        ([one], _) => Some(*one),
        ([], [only]) => Some(only),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tv(name: &str, host: &str) -> Tv {
        Tv {
            name: name.into(),
            host: host.into(),
            addr: "10.0.0.2:49152".parse().unwrap(),
            id: None,
        }
    }

    #[test]
    fn the_same_host_is_the_tv() {
        let all = [
            tv("Bedroom", "Bedroom.local."),
            tv("Living Room", "Living-Room.local."),
        ];
        assert_eq!(
            pick(&all, "living-room.local", "Elsewhere").unwrap().name,
            "Living Room"
        );
    }

    #[test]
    fn the_name_decides_when_no_host_matches() {
        let all = [
            tv("Bedroom", "Bedroom-2.local."),
            tv("Living Room", "Apple-TV.local."),
        ];
        assert_eq!(
            pick(&all, "10.0.0.4", "living room").unwrap().host,
            "Apple-TV.local."
        );
    }

    #[test]
    fn an_address_finds_the_tv_at_it() {
        let all = [
            tv("Bedroom", "Bedroom.local."),
            tv("Lounge", "Lounge.local."),
        ];
        let mut other = all.clone();
        other[1].addr = "10.0.0.9:49152".parse().unwrap();
        assert_eq!(pick(&other, "10.0.0.9", "Apple TV").unwrap().name, "Lounge");
    }

    #[test]
    fn the_only_tv_is_the_one() {
        let all = [tv("slagvision", "slagvision.local.")];
        assert_eq!(pick(&all, "", "Apple TV").unwrap().name, "slagvision");
        let two = [tv("A", "a.local."), tv("B", "b.local.")];
        assert_eq!(pick(&two, "", "Apple TV"), None);
    }

    #[test]
    fn an_address_gives_its_host() {
        assert_eq!(host_of("Living-Room.local:51234"), "Living-Room.local");
        assert_eq!(host_of("Living-Room.local"), "Living-Room.local");
    }

    #[test]
    fn two_of_one_name_are_not_guessed_between() {
        let all = [
            tv("Apple TV", "Apple-TV.local."),
            tv("Apple TV", "Apple-TV-2.local."),
        ];
        assert_eq!(pick(&all, "10.0.0.4", "Apple TV"), None);
    }

    #[test]
    fn a_sealed_frame_opens_with_the_other_sides_key() {
        let k = [7u8; 32];
        let header = [8, 0, 0, 20];
        let sealed = seal(&key(&k), counter_nonce(3), &header, b"hello");
        assert_eq!(
            open(&key(&k), counter_nonce(3), &header, &sealed).unwrap(),
            b"hello"
        );
        assert!(open(&key(&k), counter_nonce(4), &header, &sealed).is_err());
    }
}
