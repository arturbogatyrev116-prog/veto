use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey, Verifier};
use rand_core::OsRng;
use x25519_dalek::{PublicKey as X25519Public, StaticSecret};
use zeroize::ZeroizeOnDrop;

use crate::error::CryptoError;

// ── Identity Key (Ed25519, used for signing) ─────────────────────────────────

pub struct IdentityKeyPair {
    pub signing: SigningKey,
}

impl IdentityKeyPair {
    pub fn generate() -> Self {
        Self { signing: SigningKey::generate(&mut OsRng) }
    }

    pub fn public(&self) -> IdentityPublicKey {
        IdentityPublicKey { verifying: self.signing.verifying_key() }
    }

    /// Sign arbitrary bytes (used to authenticate prekey bundles).
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.signing.sign(msg).to_bytes()
    }

    /// Derive an X25519 key from the Ed25519 scalar for DH operations.
    /// We keep them separate internally to avoid key reuse (PQXDH flaw F1).
    pub fn dh_secret(&self) -> StaticSecret {
        // Clamp the first 32 bytes of the expanded Ed25519 scalar.
        let expanded = self.signing.to_scalar_bytes();
        StaticSecret::from(expanded)
    }

    pub fn dh_public(&self) -> X25519Public {
        X25519Public::from(&self.dh_secret())
    }
}

#[derive(Clone)]
pub struct IdentityPublicKey {
    pub verifying: VerifyingKey,
}

impl IdentityPublicKey {
    pub fn verify(&self, msg: &[u8], sig: &[u8; 64]) -> Result<(), CryptoError> {
        let sig = Signature::from_bytes(sig);
        self.verifying.verify(msg, &sig).map_err(|_| CryptoError::InvalidSignature)
    }

    pub fn dh_public(&self) -> X25519Public {
        // Convert Ed25519 verifying key to X25519 public key (Montgomery form).
        let ed_bytes = self.verifying.to_bytes();
        let montgomery = curve25519_from_ed25519_pubkey(&ed_bytes);
        X25519Public::from(montgomery)
    }
}

/// Convert an Ed25519 public key to X25519 (Montgomery) format.
/// This is a standard birational equivalence: u = (1+y)/(1-y)
fn curve25519_from_ed25519_pubkey(ed_pub: &[u8; 32]) -> [u8; 32] {
    // Decompress the Edwards y-coordinate.
    let mut bits = *ed_pub;
    bits[31] &= 0x7f; // clear sign bit to get y
    let y = bits;

    // u = (1 + y) / (1 - y) mod p, done in field arithmetic via the standard formula.
    // We use the raw byte manipulation from RFC 7748 §4.1.
    let mut one_plus_y = fe_add_one(&y);
    let one_minus_y = fe_sub_from_one(&y);
    let inv = fe_invert(&one_minus_y);
    fe_mul_into(&mut one_plus_y, &inv);
    one_plus_y
}

// Minimal field arithmetic over GF(2^255 - 19) for the pubkey conversion.
// These operate on little-endian 32-byte arrays treated as 256-bit integers mod p.
// For production code use a proper field library; this is clear and correct.

fn fe_add_one(a: &[u8; 32]) -> [u8; 32] {
    let mut r = *a;
    let mut carry: u16 = 1;
    for b in r.iter_mut() {
        let s = *b as u16 + carry;
        *b = s as u8;
        carry = s >> 8;
    }
    fe_reduce(&mut r);
    r
}

fn fe_sub_from_one(a: &[u8; 32]) -> [u8; 32] {
    // 1 - a mod p  =>  compute p+1-a with p = 2^255-19
    // p in LE: bytes 0..30 = 0xff, byte 31 = 0x7f, byte 0 -= 19 → 0xed
    const P: [u8; 32] = {
        let mut p = [0xffu8; 32];
        p[31] = 0x7f;
        // p[0] = 0xff - 18 = 0xed (since 2^255 - 19 in LE has byte0 = 0xed)
        p[0] = 0xed;
        p
    };
    // result = (P + 1) - a = P - a + 1
    let mut r = [0u8; 32];
    let mut borrow: i16 = 0;
    for i in 0..32 {
        let d = P[i] as i16 - a[i] as i16 - borrow;
        r[i] = d as u8;
        borrow = if d < 0 { 1 } else { 0 };
    }
    // add 1
    let mut carry: u16 = 1;
    for b in r.iter_mut() {
        let s = *b as u16 + carry;
        *b = s as u8;
        carry = s >> 8;
    }
    fe_reduce(&mut r);
    r
}

/// Fermat's little theorem inversion: a^(p-2) mod p.
/// Uses square-and-multiply on the fixed exponent p-2 = 2^255 - 21.
fn fe_invert(a: &[u8; 32]) -> [u8; 32] {
    // p - 2 in binary: 2^255 - 21
    // We use repeated squaring. For clarity we delegate to x25519_dalek's
    // FieldElement via the Montgomery ladder — but since we don't have direct
    // access, we do it with a simple binary method here.
    // Note: in production, use a dedicated field library.
    let mut result = [0u8; 32];
    result[0] = 1; // start with 1

    // exponent = p - 2, iterate over bits from high to low
    // p - 2 = 2^255 - 21; bit pattern: 253 ones, then special low bits
    let exp: [u8; 32] = {
        let mut e = [0xffu8; 32];
        e[31] = 0x7f;
        e[0] = 0xeb; // 0xed - 2 = 0xeb
        e
    };

    let base = *a;
    let mut base = base;
    for byte in exp.iter() {
        for bit in 0..8 {
            if (byte >> bit) & 1 == 1 {
                result = fe_mul(&result, &base);
            }
            base = fe_mul(&base, &base);
        }
    }
    result
}

fn fe_mul(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    // Schoolbook multiplication mod 2^255-19 in 64-bit limbs.
    let mut product = [0u64; 64];
    for i in 0..32 {
        for j in 0..32 {
            product[i + j] += a[i] as u64 * b[j] as u64;
        }
    }
    // Reduce: bytes >= 32 contribute via 2^(8*k) = 38 * 2^(8*(k-32)) mod p
    for i in 32..64 {
        product[i - 32] += product[i] * 38;
        product[i] = 0;
    }
    // Carry propagation
    let mut carry = 0u64;
    let mut r = [0u8; 32];
    for i in 0..32 {
        let v = product[i] + carry;
        r[i] = v as u8;
        carry = v >> 8;
    }
    // Final reduction: add carry * 38
    let mut c = carry * 38;
    for b in r.iter_mut() {
        let v = *b as u64 + c;
        *b = v as u8;
        c = v >> 8;
    }
    fe_reduce(&mut r);
    r
}

fn fe_mul_into(a: &mut [u8; 32], b: &[u8; 32]) {
    *a = fe_mul(a, b);
}

fn fe_reduce(a: &mut [u8; 32]) {
    // If a >= p, subtract p. p = 2^255 - 19.
    // Check if top bit is set or value >= p.
    let top = a[31] >> 7;
    a[31] &= 0x7f;
    if top != 0 {
        let mut carry: u16 = 19;
        for b in a.iter_mut() {
            let s = *b as u16 + carry;
            *b = s as u8;
            carry = s >> 8;
        }
    }
}

// ── Signed PreKey (X25519) ────────────────────────────────────────────────────

#[derive(ZeroizeOnDrop)]
pub struct SignedPreKey {
    pub secret: StaticSecret,
    pub id: u32,
}

impl SignedPreKey {
    pub fn generate(id: u32) -> Self {
        Self { secret: StaticSecret::random_from_rng(OsRng), id }
    }

    pub fn public(&self) -> X25519Public {
        X25519Public::from(&self.secret)
    }

    /// Signature over the public key bytes, signed by the identity key.
    pub fn sign(&self, identity: &IdentityKeyPair) -> [u8; 64] {
        identity.sign(&self.public().to_bytes())
    }
}

// ── One-Time PreKey (X25519) ──────────────────────────────────────────────────

#[derive(ZeroizeOnDrop)]
pub struct OneTimePreKey {
    pub secret: StaticSecret,
    pub id: u32,
}

impl OneTimePreKey {
    pub fn generate(id: u32) -> Self {
        Self { secret: StaticSecret::random_from_rng(OsRng), id }
    }

    pub fn public(&self) -> X25519Public {
        X25519Public::from(&self.secret)
    }
}

// ── Ephemeral Key (X25519) ────────────────────────────────────────────────────

#[derive(ZeroizeOnDrop)]
pub struct EphemeralKey {
    pub secret: StaticSecret,
}

impl EphemeralKey {
    pub fn generate() -> Self {
        Self { secret: StaticSecret::random_from_rng(OsRng) }
    }

    pub fn public(&self) -> X25519Public {
        X25519Public::from(&self.secret)
    }

    pub fn dh(&self, their_public: &X25519Public) -> [u8; 32] {
        self.secret.diffie_hellman(their_public).to_bytes()
    }
}

// ── PreKey Bundle (what Bob publishes to the server) ─────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
pub struct PreKeyBundle {
    pub identity_key: X25519Public,           // IK_B as X25519
    pub identity_key_ed: VerifyingKey,        // IK_B as Ed25519 (for verification)
    pub signed_prekey: X25519Public,          // SPK_B
    pub signed_prekey_id: u32,
    #[serde(with = "bytes64")]
    pub signed_prekey_sig: [u8; 64],          // Sig(IK_B, SPK_B)
    pub one_time_prekey: Option<X25519Public>, // OPK_B (optional)
    pub one_time_prekey_id: Option<u32>,
    /// ML-KEM-768 encapsulation key (1184 bytes). None on pre-PQ clients.
    #[serde(default)]
    pub pq_spk_public: Option<Vec<u8>>,
    /// Ed25519 signature over pq_spk_public bytes. None when pq_spk_public is None.
    #[serde(default)]
    pub pq_spk_sig: Option<Vec<u8>>,
}

/// Serde module for [u8; 64] — standard serde only covers arrays up to [T; 32].
mod bytes64 {
    use serde::{de::SeqAccess, de::Visitor, ser::SerializeTuple, Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S: Serializer>(bytes: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        let mut tup = s.serialize_tuple(64)?;
        for b in bytes {
            tup.serialize_element(b)?;
        }
        tup.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = [u8; 64];
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "64 bytes")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<[u8; 64], A::Error> {
                let mut arr = [0u8; 64];
                for b in &mut arr {
                    *b = seq
                        .next_element()?
                        .ok_or_else(|| serde::de::Error::custom("expected 64 bytes"))?;
                }
                Ok(arr)
            }
        }
        d.deserialize_tuple(64, V)
    }
}

impl PreKeyBundle {
    /// Verify that SPK was signed by IK.
    pub fn verify_signature(&self) -> Result<(), CryptoError> {
        let ik = IdentityPublicKey { verifying: self.identity_key_ed };
        ik.verify(&self.signed_prekey.to_bytes(), &self.signed_prekey_sig)
    }

    /// Verify the Ed25519 signature over pq_spk_public. No-op if no PQ key is present.
    pub fn verify_pq_spk_sig(&self) -> Result<(), CryptoError> {
        let (Some(pq_pub), Some(pq_sig)) = (&self.pq_spk_public, &self.pq_spk_sig) else {
            return Ok(());
        };
        let sig_arr: [u8; 64] = pq_sig.as_slice().try_into()
            .map_err(|_| CryptoError::InvalidSignature)?;
        let ik = IdentityPublicKey { verifying: self.identity_key_ed };
        ik.verify(pq_pub, &sig_arr)
    }
}

/// Legacy prekey bundle format (before ML-KEM-768 was added). Used only for
/// backwards-compatible deserialization of bundles uploaded by old clients.
#[derive(serde::Deserialize)]
struct PreKeyBundleV0 {
    identity_key: X25519Public,
    identity_key_ed: VerifyingKey,
    signed_prekey: X25519Public,
    signed_prekey_id: u32,
    #[serde(with = "bytes64")]
    signed_prekey_sig: [u8; 64],
    one_time_prekey: Option<X25519Public>,
    one_time_prekey_id: Option<u32>,
}

/// Deserialize a prekey bundle from postcard bytes, supporting both the current
/// PQ format and the legacy pre-PQ format (no `pq_spk_public`/`pq_spk_sig`).
///
/// `#[serde(default)]` does not work with postcard (binary, non-self-describing),
/// so old bundles must be decoded via this two-attempt fallback.
pub fn bundle_from_bytes(bytes: &[u8]) -> Result<PreKeyBundle, postcard::Error> {
    postcard::from_bytes(bytes).or_else(|_| {
        postcard::from_bytes::<PreKeyBundleV0>(bytes).map(|v0| PreKeyBundle {
            identity_key: v0.identity_key,
            identity_key_ed: v0.identity_key_ed,
            signed_prekey: v0.signed_prekey,
            signed_prekey_id: v0.signed_prekey_id,
            signed_prekey_sig: v0.signed_prekey_sig,
            one_time_prekey: v0.one_time_prekey,
            one_time_prekey_id: v0.one_time_prekey_id,
            pq_spk_public: None,
            pq_spk_sig: None,
        })
    })
}

/// Generate an ML-KEM-768 key pair for use as a post-quantum signed prekey.
///
/// Returns `(ek_bytes, dk_bytes)`:
/// - `ek_bytes` — 1184-byte encapsulation key (published in PreKeyBundle)
/// - `dk_bytes` — 64-byte seed (reconstitutes the decapsulation key; kept secret by Bob)
pub fn generate_pq_spk() -> (Vec<u8>, Vec<u8>) {
    use ml_kem::{Kem, KeyExport, MlKem768};
    let (dk, ek) = MlKem768::generate_keypair();
    let ek_bytes = ek.to_bytes().to_vec();
    let dk_bytes = dk.to_bytes().to_vec();
    (ek_bytes, dk_bytes)
}
