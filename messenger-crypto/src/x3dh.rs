//! X3DH — Extended Triple Diffie-Hellman key agreement (Signal spec rev.2)
//!
//! Roles:
//!   Alice = initiator (sends first message)
//!   Bob   = responder (has published a prekey bundle)
//!
//! Key material:
//!   IK_A  — Alice's identity key (X25519 form)
//!   EK_A  — Alice's ephemeral key (generated fresh per session)
//!   IK_B  — Bob's identity key (X25519 form)
//!   SPK_B — Bob's signed prekey
//!   OPK_B — Bob's one-time prekey (optional)
//!
//! DH computations:
//!   DH1 = DH(IK_A, SPK_B)
//!   DH2 = DH(EK_A, IK_B)
//!   DH3 = DH(EK_A, SPK_B)
//!   DH4 = DH(EK_A, OPK_B)  [only if OPK present]
//!
//!   SK  = KDF(DH1 || DH2 || DH3 [|| DH4] [|| pq_ss])
//!
//! PQ extension (X-Wing / PQXDH-style):
//!   If Bob's bundle includes a ML-KEM-768 SPK (pq_spk_public):
//!     Alice encapsulates → (pq_ct, pq_ss); appends pq_ss to KDF input.
//!   Bob decapsulates pq_ct using pq_spk_dk → pq_ss; same KDF input.
//!   KDF info tag: "messenger-x3dh-v2" (v1 when no PQ).

use x25519_dalek::PublicKey as X25519Public;

use crate::{
    error::CryptoError,
    kdf::hkdf_sha256,
    keys::{EphemeralKey, IdentityKeyPair, OneTimePreKey, PreKeyBundle, SignedPreKey},
};

/// 32 zero bytes used as the F salt in KDF (per Signal spec).
const F: [u8; 32] = [0xff; 32];

/// The shared secret produced by X3DH key agreement.
/// 32 bytes, used to initialise the Double Ratchet.
#[derive(Clone)]
pub struct X3dhSecret(pub [u8; 32]);

/// Header Alice sends to Bob so he can reconstruct the shared secret.
/// Intentionally unchanged from v1 — pq_ct is carried separately in InitEnvelope.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct X3dhHeader {
    /// Alice's identity key (X25519 public, for Bob to do DH).
    #[serde(with = "x25519_bytes")]
    pub ik_a: X25519Public,
    /// Alice's ephemeral key.
    #[serde(with = "x25519_bytes")]
    pub ek_a: X25519Public,
    /// Which of Bob's signed prekeys was used.
    pub spk_id: u32,
    /// Which one-time prekey was consumed (if any).
    pub opk_id: Option<u32>,
}

mod x25519_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use x25519_dalek::PublicKey as X25519Public;

    pub fn serialize<S: Serializer>(key: &X25519Public, s: S) -> Result<S::Ok, S::Error> {
        key.to_bytes().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<X25519Public, D::Error> {
        let bytes = <[u8; 32]>::deserialize(d)?;
        Ok(X25519Public::from(bytes))
    }
}

// ── PQ helpers ────────────────────────────────────────────────────────────────

/// ML-KEM-768 encapsulation.
///
/// Returns `(ct_bytes: Vec<u8>, shared_secret: [u8; 32])` on success.
fn pq_encapsulate(ek_bytes: &[u8]) -> Result<(Vec<u8>, [u8; 32]), CryptoError> {
    use ml_kem::{Encapsulate, EncapsulationKey, MlKem768, TryKeyInit};

    let ek = EncapsulationKey::<MlKem768>::new_from_slice(ek_bytes)
        .map_err(|_| CryptoError::Pq)?;
    let (ct, ss) = ek.encapsulate();

    let ss_bytes: [u8; 32] = ss.as_slice().try_into().map_err(|_| CryptoError::Pq)?;
    Ok((ct.to_vec(), ss_bytes))
}

/// ML-KEM-768 decapsulation.
///
/// `dk_bytes` is the 64-byte seed stored by Bob. Returns the 32-byte shared secret.
fn pq_decapsulate(dk_bytes: &[u8], ct_bytes: &[u8]) -> Result<[u8; 32], CryptoError> {
    use ml_kem::{Decapsulate, DecapsulationKey, KeyInit, MlKem768};

    let dk = DecapsulationKey::<MlKem768>::new_from_slice(dk_bytes)
        .map_err(|_| CryptoError::Pq)?;
    let ss = dk
        .decapsulate_slice(ct_bytes)
        .map_err(|_| CryptoError::Pq)?;

    ss.as_slice().try_into().map_err(|_| CryptoError::Pq)
}

// ── Alice (initiator) ─────────────────────────────────────────────────────────

/// Perform X3DH as Alice, optionally with ML-KEM-768 post-quantum extension.
///
/// Returns `(shared_secret, header_to_send_to_bob, pq_ct)`.
/// `pq_ct` is `Some(bytes)` when Bob's bundle advertises a PQ SPK; otherwise `None`.
/// The caller must include `pq_ct` in the `InitEnvelope` it sends to Bob.
pub fn x3dh_send(
    alice_identity: &IdentityKeyPair,
    bob_bundle: &PreKeyBundle,
) -> Result<(X3dhSecret, X3dhHeader, Option<Vec<u8>>), CryptoError> {
    // 1. Verify Bob's X25519 SPK signature before using it.
    bob_bundle.verify_signature()?;

    // 2. Generate Alice's ephemeral key pair.
    let ek_a = EphemeralKey::generate();

    // 3. DH computations.
    let dh1 = alice_identity.dh_secret().diffie_hellman(&bob_bundle.signed_prekey).to_bytes();
    let dh2 = ek_a.dh(&bob_bundle.identity_key);
    let dh3 = ek_a.dh(&bob_bundle.signed_prekey);

    // 4. Assemble KDF input.
    let mut ikm = Vec::with_capacity(32 + 32 * 5);
    ikm.extend_from_slice(&F);
    ikm.extend_from_slice(&dh1);
    ikm.extend_from_slice(&dh2);
    ikm.extend_from_slice(&dh3);

    let opk_id = if let (Some(opk_pub), Some(opk_id)) =
        (bob_bundle.one_time_prekey, bob_bundle.one_time_prekey_id)
    {
        let dh4 = ek_a.dh(&opk_pub);
        ikm.extend_from_slice(&dh4);
        Some(opk_id)
    } else {
        None
    };

    // 5. Post-quantum extension: encapsulate to Bob's ML-KEM-768 SPK if present.
    let (pq_ct, use_v2) = if let Some(ref pq_ek_bytes) = bob_bundle.pq_spk_public {
        bob_bundle.verify_pq_spk_sig()?;
        let (ct_bytes, pq_ss) = pq_encapsulate(pq_ek_bytes)?;
        ikm.extend_from_slice(&pq_ss);
        (Some(ct_bytes), true)
    } else {
        (None, false)
    };

    // 6. Derive shared secret.
    let sk = kdf_x3dh(&ikm, use_v2);

    let header = X3dhHeader {
        ik_a: alice_identity.dh_public(),
        ek_a: ek_a.public(),
        spk_id: bob_bundle.signed_prekey_id,
        opk_id,
    };

    Ok((X3dhSecret(sk), header, pq_ct))
}

// ── Bob (responder) ───────────────────────────────────────────────────────────

/// Perform X3DH as Bob, given Alice's header, the PQ ciphertext from InitEnvelope,
/// and Bob's key material.
///
/// `pq_ct`          — from `InitEnvelope.pq_ct`; `None` if Alice did not use PQ.
/// `bob_pq_spk_dk`  — Bob's ML-KEM-768 decapsulation key; `None` if no PQ SPK was registered.
pub fn x3dh_receive(
    bob_identity: &IdentityKeyPair,
    bob_spk: &SignedPreKey,
    bob_opk: Option<&OneTimePreKey>,
    header: &X3dhHeader,
    pq_ct: Option<&[u8]>,
    bob_pq_spk_dk: Option<&[u8]>,
) -> Result<X3dhSecret, CryptoError> {
    let dh1 = bob_spk.secret.diffie_hellman(&header.ik_a).to_bytes();
    let dh2 = bob_identity.dh_secret().diffie_hellman(&header.ek_a).to_bytes();
    let dh3 = bob_spk.secret.diffie_hellman(&header.ek_a).to_bytes();

    let mut ikm = Vec::with_capacity(32 + 32 * 5);
    ikm.extend_from_slice(&F);
    ikm.extend_from_slice(&dh1);
    ikm.extend_from_slice(&dh2);
    ikm.extend_from_slice(&dh3);

    if let Some(opk) = bob_opk {
        let dh4 = opk.secret.diffie_hellman(&header.ek_a).to_bytes();
        ikm.extend_from_slice(&dh4);
    } else if header.opk_id.is_some() {
        return Err(CryptoError::NoOneTimePrekey);
    }

    // Post-quantum extension: decapsulate if Alice included a pq_ct.
    let use_v2 = if let Some(ct_bytes) = pq_ct {
        let dk = bob_pq_spk_dk
            .filter(|b| !b.is_empty())
            .ok_or(CryptoError::NoPqDecapKey)?;
        let pq_ss = pq_decapsulate(dk, ct_bytes)?;
        ikm.extend_from_slice(&pq_ss);
        true
    } else {
        false
    };

    Ok(X3dhSecret(kdf_x3dh(&ikm, use_v2)))
}

// ── KDF ───────────────────────────────────────────────────────────────────────

fn kdf_x3dh(ikm: &[u8], v2: bool) -> [u8; 32] {
    let mut sk = [0u8; 32];
    let info: &[u8] = if v2 { b"messenger-x3dh-v2" } else { b"messenger-x3dh-v1" };
    hkdf_sha256(ikm, None, info, &mut sk);
    sk
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{generate_pq_spk, OneTimePreKey, SignedPreKey};

    fn make_bob_bundle(
        bob_id: &IdentityKeyPair,
        spk: &SignedPreKey,
        opk: Option<&OneTimePreKey>,
    ) -> PreKeyBundle {
        PreKeyBundle {
            identity_key: bob_id.dh_public(),
            identity_key_ed: bob_id.public().verifying,
            signed_prekey: spk.public(),
            signed_prekey_id: spk.id,
            signed_prekey_sig: spk.sign(bob_id),
            one_time_prekey: opk.map(|k| k.public()),
            one_time_prekey_id: opk.map(|k| k.id),
            pq_spk_public: None,
            pq_spk_sig: None,
        }
    }

    #[test]
    fn x3dh_without_opk_produces_equal_secrets() {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);

        let bundle = make_bob_bundle(&bob_id, &bob_spk, None);

        let (alice_sk, header, pq_ct) = x3dh_send(&alice_id, &bundle).unwrap();
        let bob_sk = x3dh_receive(&bob_id, &bob_spk, None, &header, pq_ct.as_deref(), None).unwrap();

        assert_eq!(alice_sk.0, bob_sk.0, "shared secrets must match");
    }

    #[test]
    fn x3dh_with_opk_produces_equal_secrets() {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);
        let bob_opk = OneTimePreKey::generate(42);

        let bundle = make_bob_bundle(&bob_id, &bob_spk, Some(&bob_opk));

        let (alice_sk, header, pq_ct) = x3dh_send(&alice_id, &bundle).unwrap();
        let bob_sk = x3dh_receive(&bob_id, &bob_spk, Some(&bob_opk), &header, pq_ct.as_deref(), None).unwrap();

        assert_eq!(alice_sk.0, bob_sk.0, "shared secrets must match (with OPK)");
    }

    #[test]
    fn x3dh_different_parties_get_different_secrets() {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let eve_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);

        let bundle = make_bob_bundle(&bob_id, &bob_spk, None);
        let (alice_sk, header, pq_ct) = x3dh_send(&alice_id, &bundle).unwrap();

        let eve_sk = x3dh_receive(&eve_id, &bob_spk, None, &header, pq_ct.as_deref(), None).unwrap();
        assert_ne!(alice_sk.0, eve_sk.0, "impostor must not get Alice's secret");
    }

    #[test]
    fn x3dh_invalid_signature_rejected() {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);
        let mut bundle = make_bob_bundle(&bob_id, &bob_spk, None);

        bundle.signed_prekey_sig[0] ^= 0xff;

        let result = x3dh_send(&alice_id, &bundle);
        assert!(
            matches!(result, Err(CryptoError::InvalidSignature)),
            "tampered signature must be rejected"
        );
    }

    // ── PQ / X-Wing tests ────────────────────────────────────────────────────

    #[test]
    fn x3dh_xwing_produces_equal_secrets() {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);
        let (pq_ek, pq_dk) = generate_pq_spk();
        let pq_sig: Vec<u8> = bob_id.sign(&pq_ek).to_vec();

        let bundle = PreKeyBundle {
            identity_key: bob_id.dh_public(),
            identity_key_ed: bob_id.public().verifying,
            signed_prekey: bob_spk.public(),
            signed_prekey_id: bob_spk.id,
            signed_prekey_sig: bob_spk.sign(&bob_id),
            one_time_prekey: None,
            one_time_prekey_id: None,
            pq_spk_public: Some(pq_ek),
            pq_spk_sig: Some(pq_sig),
        };

        let (alice_sk, header, pq_ct) = x3dh_send(&alice_id, &bundle).unwrap();
        assert!(pq_ct.is_some(), "pq_ct must be produced when bundle has pq_spk");

        let bob_sk = x3dh_receive(
            &bob_id, &bob_spk, None, &header, pq_ct.as_deref(), Some(&pq_dk),
        ).unwrap();

        assert_eq!(alice_sk.0, bob_sk.0, "X-Wing shared secrets must match");
    }

    #[test]
    fn x3dh_xwing_no_pq_in_bundle_uses_v1() {
        // Bundle without pq_spk → pq_ct=None → v1 KDF used on both sides.
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);

        let bundle = make_bob_bundle(&bob_id, &bob_spk, None);
        let (alice_sk, header, pq_ct) = x3dh_send(&alice_id, &bundle).unwrap();
        assert!(pq_ct.is_none());

        let bob_sk = x3dh_receive(&bob_id, &bob_spk, None, &header, None, None).unwrap();
        assert_eq!(alice_sk.0, bob_sk.0);
    }

    #[test]
    fn x3dh_pq_ct_without_dk_returns_error() {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);
        let (pq_ek, _pq_dk) = generate_pq_spk();
        let pq_sig: Vec<u8> = bob_id.sign(&pq_ek).to_vec();

        let bundle = PreKeyBundle {
            identity_key: bob_id.dh_public(),
            identity_key_ed: bob_id.public().verifying,
            signed_prekey: bob_spk.public(),
            signed_prekey_id: bob_spk.id,
            signed_prekey_sig: bob_spk.sign(&bob_id),
            one_time_prekey: None,
            one_time_prekey_id: None,
            pq_spk_public: Some(pq_ek),
            pq_spk_sig: Some(pq_sig),
        };

        let (_, header, pq_ct) = x3dh_send(&alice_id, &bundle).unwrap();
        // Bob claims to have no PQ DK → must error.
        let result = x3dh_receive(&bob_id, &bob_spk, None, &header, pq_ct.as_deref(), None);
        assert!(
            matches!(result, Err(CryptoError::NoPqDecapKey)),
            "missing PQ DK must return NoPqDecapKey"
        );
    }

    #[test]
    fn x3dh_xwing_tampered_pq_sig_rejected() {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);
        let (pq_ek, _) = generate_pq_spk();
        let mut pq_sig: Vec<u8> = bob_id.sign(&pq_ek).to_vec();
        pq_sig[0] ^= 0xff; // corrupt signature

        let bundle = PreKeyBundle {
            identity_key: bob_id.dh_public(),
            identity_key_ed: bob_id.public().verifying,
            signed_prekey: bob_spk.public(),
            signed_prekey_id: bob_spk.id,
            signed_prekey_sig: bob_spk.sign(&bob_id),
            one_time_prekey: None,
            one_time_prekey_id: None,
            pq_spk_public: Some(pq_ek),
            pq_spk_sig: Some(pq_sig),
        };

        let result = x3dh_send(&alice_id, &bundle);
        assert!(
            matches!(result, Err(CryptoError::InvalidSignature)),
            "tampered PQ SPK signature must be rejected"
        );
    }
}
