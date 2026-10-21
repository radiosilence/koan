//! The client half of SRP-6a as HomeKit pair-setup uses it: the 3072-bit
//! group of RFC 5054, SHA-512, the user `Pair-Setup` and the PIN as the
//! password. Values are hashed in their shortest big-endian form except where
//! RFC 5054 pads them, as the TV (and pyatv) expect.

use num_bigint::BigUint;
use sha2::{Digest, Sha512};

const N_HEX: &str = "\
FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74\
020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437\
4FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED\
EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05\
98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB\
9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3B\
E39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF695581718\
3995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D04507A33\
A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7\
ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864\
D87602733EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E2\
08E24FA074E5AB3143DB5BFCE0FD108E4B82D120A93AD2CAFFFFFFFFFFFFFFFF";
const G: u32 = 5;
const USER: &[u8] = b"Pair-Setup";

fn n() -> BigUint {
    BigUint::parse_bytes(N_HEX.as_bytes(), 16).expect("the group's prime")
}

fn hash(parts: &[&[u8]]) -> Vec<u8> {
    let mut h = Sha512::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().to_vec()
}

fn int(bytes: &[u8]) -> BigUint {
    BigUint::from_bytes_be(bytes)
}

/// `v` as the group's length, zeros first.
fn pad(v: &BigUint, len: usize) -> Vec<u8> {
    let b = v.to_bytes_be();
    let mut out = vec![0; len.saturating_sub(b.len())];
    out.extend_from_slice(&b);
    out
}

/// What the client sends and keeps once it has the server's salt and key.
pub struct Exchange {
    /// `A`, sent to the TV.
    pub public: Vec<u8>,
    /// `M1`, sent to the TV.
    pub proof: Vec<u8>,
    /// `K`, the session key both sides derive.
    pub key: Vec<u8>,
    /// `M2`, which the TV's answer must equal.
    expected: Vec<u8>,
}

impl Exchange {
    /// Run the client's side against the TV's `salt` and public key `b`, with
    /// the PIN and a random private value. `None` for a `b` that is zero
    /// modulo the prime, which a TV never sends.
    pub fn new(pin: &str, salt: &[u8], b: &[u8], private: &[u8]) -> Option<Self> {
        let n = n();
        let len = n.to_bytes_be().len();
        let g = BigUint::from(G);
        let b_int = int(b);
        if (&b_int % &n) == BigUint::ZERO {
            return None;
        }
        let a = int(private);
        let a_pub = g.modpow(&a, &n);
        let k = int(&hash(&[&n.to_bytes_be(), &pad(&g, len)]));
        let u = int(&hash(&[&pad(&a_pub, len), &pad(&b_int, len)]));
        let inner = hash(&[USER, b":", pin.as_bytes()]);
        let x = int(&hash(&[&int(salt).to_bytes_be(), &inner]));
        let v = g.modpow(&x, &n);
        let base = (&b_int + &n - (&k * &v) % &n) % &n;
        let s = base.modpow(&(&a + &u * &x), &n);
        let key = hash(&[&s.to_bytes_be()]);

        let hn = hash(&[&n.to_bytes_be()]);
        let hg = hash(&[&g.to_bytes_be()]);
        let xor: Vec<u8> = hn.iter().zip(&hg).map(|(a, b)| a ^ b).collect();
        let proof = hash(&[
            &int(&xor).to_bytes_be(),
            &int(&hash(&[USER])).to_bytes_be(),
            &int(salt).to_bytes_be(),
            &a_pub.to_bytes_be(),
            &b_int.to_bytes_be(),
            &key,
        ]);
        let expected = hash(&[&a_pub.to_bytes_be(), &proof, &key]);
        Some(Self {
            public: a_pub.to_bytes_be(),
            proof,
            key,
            expected,
        })
    }

    /// Whether the TV's `M2` shows it knows the PIN too.
    pub fn verify(&self, server_proof: &[u8]) -> bool {
        use subtle::ConstantTimeEq;
        bool::from(self.expected.as_slice().ct_eq(server_proof))
    }
}

/// The server's side, for tests: the verifier for `pin` and the key it
/// derives from the client's `A`.
#[cfg(test)]
pub(crate) fn server(
    pin: &str,
    salt: &[u8],
    private: &[u8],
) -> (Vec<u8>, impl Fn(&[u8]) -> Vec<u8>) {
    let n = n();
    let len = n.to_bytes_be().len();
    let g = BigUint::from(G);
    let inner = hash(&[USER, b":", pin.as_bytes()]);
    let x = int(&hash(&[&int(salt).to_bytes_be(), &inner]));
    let v = g.modpow(&x, &n);
    let k = int(&hash(&[&n.to_bytes_be(), &pad(&g, len)]));
    let b = int(private);
    let b_pub = (&k * &v + g.modpow(&b, &n)) % &n;
    let b_bytes = b_pub.to_bytes_be();
    let key = move |a: &[u8]| {
        let a_int = int(a);
        let u = int(&hash(&[&pad(&a_int, len), &pad(&b_pub, len)]));
        let s = (&a_int * v.modpow(&u, &n)).modpow(&b, &n);
        hash(&[&s.to_bytes_be()])
    };
    (b_bytes, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sides_derive_one_key_from_one_pin() {
        let salt = [0x5a; 16];
        let (b, server_key) = server("1026", &salt, &[0x42; 32]);
        let client = Exchange::new("1026", &salt, &b, &[0x17; 32]).unwrap();
        assert_eq!(client.key, server_key(&client.public));
    }

    #[test]
    fn a_wrong_pin_derives_another_key() {
        let salt = [0x5a; 16];
        let (b, server_key) = server("1026", &salt, &[0x42; 32]);
        let client = Exchange::new("5094", &salt, &b, &[0x17; 32]).unwrap();
        assert_ne!(client.key, server_key(&client.public));
    }

    /// srptools, which pyatv pairs with, given the same salt, PIN and
    /// private values.
    #[test]
    fn the_proof_and_key_match_pyatvs() {
        let hex = |s: &str| {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect::<Vec<u8>>()
        };
        let b = hex("\
c22e2b1c850c4d2ae0bf9ed8b5c516ea0dec3ad122abe36b9ce593e461385d84\
cb13fb644dedbfd1162cea8cb18f60b6e9b0df5429a1e2d81d6ad3458cc7b056\
301608437ae4ae9002fbfd03dbe7c072180e34a1226fa7131d22a23264c55b26\
259c9b778641aece991a02c871f557fed03b65a12c4a4d41b064593f564a60a0\
5be16093330a94afe9b52f354dc7d4455b3ead0c2802e919abc74ea7ca383689\
2091b3bab621d2d4df4af5d0d0b5e02bbfb3bae2e98f43efec099355411447b8\
619d9755059642e17bf8b417a02ce1679230efd5f4bdcfbedd5fd77c03e77558\
d92bc18870d603a3860bf033590fe81253d409685ecf9b170ebba3973fae2984\
d29dcc199921c327f2a95fc1002e0d437d25a2f15f3dad6ad022cb57926b88c7\
ff92252dfe757d1c6c4ea1614da5840e8d2e10079a3871dd7a912552e2e15fbb\
7f45d69cf2e43491274f78e2e4f29d780bdb66ea172b593d4080055c80437de4\
ccc86bd33570317716714d6280a2a82365c50793a26bf38905830430d7cd41c6");
        let client = Exchange::new("1026", &[0x5a; 16], &b, &[0x17; 32]).unwrap();
        assert_eq!(
            client.proof,
            hex("\
8d4f1ddd5a0d47a27db0fadd94d84cc0f27e593b8d9d3c55effb35f28596f36b\
575e7a8468e6aaba17606ee1ce89e228f8ee8b7f1dd9b48252b73ddb302a8220")
        );
        assert_eq!(
            client.key,
            hex("\
0f306ad27976a11255cb7a030d512c11721a5124e0144af9943b81035c63201d\
e37730d424f01d3082557b5b84c5a7639b952505ae17d0bb35a6de9176ba3cf9")
        );
    }

    #[test]
    fn a_zero_key_from_the_server_is_refused() {
        assert!(Exchange::new("1026", &[1; 16], &[0], &[0x17; 32]).is_none());
    }
}
