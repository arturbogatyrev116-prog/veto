//! Double Ratchet Algorithm
//!
//! Combines two ratchets:
//!   1. Symmetric-key ratchet (KDF chain) — advances with every message,
//!      gives forward secrecy within a chain.
//!   2. Diffie-Hellman ratchet — advances when a new DH public key is received,
//!      gives post-compromise security (break-in recovery).
//!
//! State machine:
//!   - Each side holds a root key (RK), a sending chain key (CKs),
//!     a receiving chain key (CKr), current DH ratchet key pair,
//!     and the peer's last known DH public key.
//!   - Sending: advance CKs → produce message key → encrypt.
//!   - Receiving: if new DH key in header → DH ratchet step first,
//!     then advance CKr → message key → decrypt.
//!   - Out-of-order: skip ahead and cache unused message keys.

use std::collections::HashMap;

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use rand_core::OsRng;
use x25519_dalek::{PublicKey as X25519Public, StaticSecret};
use zeroize::ZeroizeOnDrop;

use crate::{
    error::CryptoError,
    kdf::{kdf_ck, kdf_rk, message_keys},
    x3dh::X3dhSecret,
};

/// Maximum number of message keys we'll cache for out-of-order delivery.
const MAX_SKIP: u32 = 1000;

/// Encrypted message produced by the ratchet.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Message {
    /// Sender's current DH ratchet public key.
    pub dh_pub: [u8; 32],
    /// Previous sending chain length (needed for OOO recovery).
    pub prev_chain_len: u32,
    /// Index of this message within the current sending chain.
    pub msg_index: u32,
    /// AEAD ciphertext (ChaCha20-Poly1305, includes 16-byte tag).
    pub ciphertext: Vec<u8>,
}

/// (dh_pub bytes, message_index) → message key
type SkippedKeys = HashMap<([u8; 32], u32), [u8; 32]>;

// ── Ratchet State ─────────────────────────────────────────────────────────────

#[derive(ZeroizeOnDrop, serde::Serialize, serde::Deserialize)]
pub struct RatchetState {
    root_key: [u8; 32],
    /// Sending chain key (None until first send after receiving a DH key).
    chain_key_send: Option<[u8; 32]>,
    /// Receiving chain key.
    chain_key_recv: Option<[u8; 32]>,
    /// Our current DH ratchet key pair.
    dh_self: StaticSecret,
    dh_self_pub: [u8; 32],
    /// Peer's last received DH public key.
    dh_remote: Option<[u8; 32]>,
    /// Send message counter in current chain.
    send_count: u32,
    /// Receive message counter in current chain.
    recv_count: u32,
    /// Previous send chain length (for OOO messages).
    prev_send_count: u32,
    /// Cached message keys for out-of-order messages.
    #[zeroize(skip)]
    skipped: SkippedKeys,
}

impl RatchetState {
    // ── Initialisation ────────────────────────────────────────────────────────

    /// Alice initialises the ratchet after X3DH.
    /// She knows Bob's SPK public key, which becomes the first remote DH key.
    pub fn init_alice(shared_secret: &X3dhSecret, bob_dh_pub: X25519Public) -> Self {
        let dh_self = StaticSecret::random_from_rng(OsRng);
        let dh_self_pub = X25519Public::from(&dh_self).to_bytes();

        // First DH ratchet step using X3DH secret as root key.
        let dh_out = dh_self.diffie_hellman(&bob_dh_pub).to_bytes();
        let (root_key, chain_key_send) = kdf_rk(&shared_secret.0, &dh_out);

        RatchetState {
            root_key,
            chain_key_send: Some(chain_key_send),
            chain_key_recv: None,
            dh_self,
            dh_self_pub,
            dh_remote: Some(bob_dh_pub.to_bytes()),
            send_count: 0,
            recv_count: 0,
            prev_send_count: 0,
            skipped: HashMap::new(),
        }
    }

    /// Bob initialises the ratchet after X3DH.
    /// He uses his SPK as the initial DH ratchet key.
    pub fn init_bob(shared_secret: &X3dhSecret, bob_spk_secret: StaticSecret) -> Self {
        let dh_self_pub = X25519Public::from(&bob_spk_secret).to_bytes();
        RatchetState {
            root_key: shared_secret.0,
            chain_key_send: None,
            chain_key_recv: None,
            dh_self: bob_spk_secret,
            dh_self_pub,
            dh_remote: None,
            send_count: 0,
            recv_count: 0,
            prev_send_count: 0,
            skipped: HashMap::new(),
        }
    }

    // ── Encryption ────────────────────────────────────────────────────────────

    pub fn encrypt(&mut self, plaintext: &[u8], associated_data: &[u8]) -> Message {
        let (new_ck, mk) = kdf_ck(self.chain_key_send.as_ref().expect("sending chain not initialised"));
        self.chain_key_send = Some(new_ck);

        let header = MessageHeader {
            dh_pub: self.dh_self_pub,
            prev_chain_len: self.prev_send_count,
            msg_index: self.send_count,
        };
        self.send_count += 1;

        let ciphertext = aead_encrypt(&mk, &header.encode(associated_data), plaintext);

        Message {
            dh_pub: header.dh_pub,
            prev_chain_len: header.prev_chain_len,
            msg_index: header.msg_index,
            ciphertext,
        }
    }

    // ── Decryption ────────────────────────────────────────────────────────────

    pub fn decrypt(&mut self, msg: &Message, associated_data: &[u8]) -> Result<Vec<u8>, CryptoError> {
        // Check if we have a cached key for an out-of-order message.
        if let Some(mk) = self.skipped.remove(&(msg.dh_pub, msg.msg_index)) {
            let header = MessageHeader {
                dh_pub: msg.dh_pub,
                prev_chain_len: msg.prev_chain_len,
                msg_index: msg.msg_index,
            };
            return aead_decrypt(&mk, &header.encode(associated_data), &msg.ciphertext);
        }

        let dh_changed = self.dh_remote.as_ref() != Some(&msg.dh_pub);

        if dh_changed {
            // New DH ratchet key from remote — perform a DH ratchet step.
            // First, skip any remaining messages in the previous receive chain.
            self.skip_message_keys(msg.prev_chain_len)?;
            self.dh_ratchet_step(&msg.dh_pub)?;
        }

        // Advance the receive chain to the message index.
        self.skip_message_keys(msg.msg_index)?;

        let ck = self.chain_key_recv.as_ref().ok_or(CryptoError::DecryptionFailed)?;
        let (new_ck, mk) = kdf_ck(ck);
        self.chain_key_recv = Some(new_ck);
        self.recv_count += 1;

        let header = MessageHeader {
            dh_pub: msg.dh_pub,
            prev_chain_len: msg.prev_chain_len,
            msg_index: msg.msg_index,
        };
        aead_decrypt(&mk, &header.encode(associated_data), &msg.ciphertext)
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Cache message keys up to `until` (exclusive) in the receive chain.
    fn skip_message_keys(&mut self, until: u32) -> Result<(), CryptoError> {
        if until.saturating_sub(self.recv_count) > MAX_SKIP {
            return Err(CryptoError::TooManySkippedMessages { max: MAX_SKIP });
        }
        while self.recv_count < until {
            let ck = self.chain_key_recv.as_ref().ok_or(CryptoError::DecryptionFailed)?;
            let (new_ck, mk) = kdf_ck(ck);
            let dh_key = self.dh_remote.ok_or(CryptoError::DecryptionFailed)?;
            self.skipped.insert((dh_key, self.recv_count), mk);
            self.chain_key_recv = Some(new_ck);
            self.recv_count += 1;
        }
        Ok(())
    }

    /// Perform a DH ratchet step: update receive chain, generate new DH pair,
    /// update send chain.
    fn dh_ratchet_step(&mut self, remote_dh_pub_bytes: &[u8; 32]) -> Result<(), CryptoError> {
        let remote_pub = X25519Public::from(*remote_dh_pub_bytes);

        // Step 1: receive chain from new remote key.
        let dh_out = self.dh_self.diffie_hellman(&remote_pub).to_bytes();
        let (rk, ck_recv) = kdf_rk(&self.root_key, &dh_out);
        self.root_key = rk;
        self.chain_key_recv = Some(ck_recv);
        self.dh_remote = Some(*remote_dh_pub_bytes);
        self.recv_count = 0;

        // Step 2: generate new DH key pair and derive send chain.
        let new_dh = StaticSecret::random_from_rng(OsRng);
        let new_dh_pub = X25519Public::from(&new_dh).to_bytes();
        let dh_out2 = new_dh.diffie_hellman(&remote_pub).to_bytes();
        let (rk2, ck_send) = kdf_rk(&self.root_key, &dh_out2);
        self.root_key = rk2;
        self.chain_key_send = Some(ck_send);
        self.prev_send_count = self.send_count;
        self.send_count = 0;
        self.dh_self = new_dh;
        self.dh_self_pub = new_dh_pub;

        Ok(())
    }
}

// ── Message header (encoded into AEAD associated data) ────────────────────────

struct MessageHeader {
    dh_pub: [u8; 32],
    prev_chain_len: u32,
    msg_index: u32,
}

impl MessageHeader {
    /// Encode header + caller's associated_data into bytes for AEAD.
    fn encode(&self, associated_data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + 4 + 4 + associated_data.len());
        out.extend_from_slice(&self.dh_pub);
        out.extend_from_slice(&self.prev_chain_len.to_be_bytes());
        out.extend_from_slice(&self.msg_index.to_be_bytes());
        out.extend_from_slice(associated_data);
        out
    }
}

// ── AEAD (ChaCha20-Poly1305) ──────────────────────────────────────────────────

fn aead_encrypt(mk: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let (enc_key, _, iv) = message_keys(mk);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&enc_key));
    let nonce = Nonce::from_slice(&iv);
    // chacha20poly1305 encrypt_in_place_detached needs payload; use encrypt with aad via Payload.
    cipher
        .encrypt(nonce, chacha20poly1305::aead::Payload { msg: plaintext, aad })
        .expect("encryption cannot fail with valid key/nonce sizes")
}

fn aead_decrypt(mk: &[u8; 32], aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let (enc_key, _, iv) = message_keys(mk);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&enc_key));
    let nonce = Nonce::from_slice(&iv);
    cipher
        .decrypt(nonce, chacha20poly1305::aead::Payload { msg: ciphertext, aad })
        .map_err(|_| CryptoError::DecryptionFailed)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use x25519_dalek::StaticSecret;

    use super::*;
    use crate::{
        keys::{IdentityKeyPair, PreKeyBundle, SignedPreKey},
        x3dh::{x3dh_receive, x3dh_send},
    };

    #[test]
    fn alice_sends_bob_receives() {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);

        // Keep the secret for Bob's init.
        let bob_spk_bytes: [u8; 32] = bob_spk.secret.to_bytes();

        let bundle = PreKeyBundle {
            identity_key: bob_id.dh_public(),
            identity_key_ed: bob_id.public().verifying,
            signed_prekey: bob_spk.public(),
            signed_prekey_id: bob_spk.id,
            signed_prekey_sig: bob_spk.sign(&bob_id),
            one_time_prekey: None,
            one_time_prekey_id: None,
            pq_spk_public: None,
            pq_spk_sig: None,
        };

        let (alice_sk, header, pq_ct) = x3dh_send(&alice_id, &bundle).unwrap();
        let bob_sk = x3dh_receive(&bob_id, &bob_spk, None, &header, pq_ct.as_deref(), None).unwrap();

        let bob_spk_secret = StaticSecret::from(bob_spk_bytes);
        let mut alice = RatchetState::init_alice(&alice_sk, bob_spk.public());
        let mut bob = RatchetState::init_bob(&bob_sk, bob_spk_secret);

        let plaintext = b"Hello, Bob! This is a secret message.";
        let ad = b"alice-bob-session-v1";

        let msg = alice.encrypt(plaintext, ad);
        let decrypted = bob.decrypt(&msg, ad).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn bidirectional_multi_message() {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);
        let bob_spk_bytes: [u8; 32] = bob_spk.secret.to_bytes();

        let bundle = PreKeyBundle {
            identity_key: bob_id.dh_public(),
            identity_key_ed: bob_id.public().verifying,
            signed_prekey: bob_spk.public(),
            signed_prekey_id: bob_spk.id,
            signed_prekey_sig: bob_spk.sign(&bob_id),
            one_time_prekey: None,
            one_time_prekey_id: None,
            pq_spk_public: None,
            pq_spk_sig: None,
        };

        let (alice_sk, header, pq_ct) = x3dh_send(&alice_id, &bundle).unwrap();
        let bob_sk = x3dh_receive(&bob_id, &bob_spk, None, &header, pq_ct.as_deref(), None).unwrap();

        let mut alice = RatchetState::init_alice(&alice_sk, bob_spk.public());
        let mut bob = RatchetState::init_bob(&bob_sk, StaticSecret::from(bob_spk_bytes));

        let ad = b"session";

        // Alice → Bob (3 messages)
        for i in 0u8..3 {
            let pt = vec![i; 16];
            let msg = alice.encrypt(&pt, ad);
            let dec = bob.decrypt(&msg, ad).unwrap();
            assert_eq!(dec, pt);
        }

        // Bob → Alice (2 messages, triggers DH ratchet on Alice's side)
        for i in 10u8..12 {
            let pt = vec![i; 16];
            let msg = bob.encrypt(&pt, ad);
            let dec = alice.decrypt(&msg, ad).unwrap();
            assert_eq!(dec, pt);
        }

        // Alice → Bob again (new DH ratchet)
        for i in 20u8..23 {
            let pt = vec![i; 16];
            let msg = alice.encrypt(&pt, ad);
            let dec = bob.decrypt(&msg, ad).unwrap();
            assert_eq!(dec, pt);
        }
    }

    #[test]
    fn out_of_order_delivery() {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);
        let bob_spk_bytes: [u8; 32] = bob_spk.secret.to_bytes();

        let bundle = PreKeyBundle {
            identity_key: bob_id.dh_public(),
            identity_key_ed: bob_id.public().verifying,
            signed_prekey: bob_spk.public(),
            signed_prekey_id: bob_spk.id,
            signed_prekey_sig: bob_spk.sign(&bob_id),
            one_time_prekey: None,
            one_time_prekey_id: None,
            pq_spk_public: None,
            pq_spk_sig: None,
        };

        let (alice_sk, header, pq_ct) = x3dh_send(&alice_id, &bundle).unwrap();
        let bob_sk = x3dh_receive(&bob_id, &bob_spk, None, &header, pq_ct.as_deref(), None).unwrap();

        let mut alice = RatchetState::init_alice(&alice_sk, bob_spk.public());
        let mut bob = RatchetState::init_bob(&bob_sk, StaticSecret::from(bob_spk_bytes));

        let ad = b"session";

        // Alice sends 3 messages but Bob receives them out of order: 2, 0, 1
        let msg0 = alice.encrypt(b"msg0", ad);
        let msg1 = alice.encrypt(b"msg1", ad);
        let msg2 = alice.encrypt(b"msg2", ad);

        assert_eq!(bob.decrypt(&msg2, ad).unwrap(), b"msg2");
        assert_eq!(bob.decrypt(&msg0, ad).unwrap(), b"msg0");
        assert_eq!(bob.decrypt(&msg1, ad).unwrap(), b"msg1");
    }

    #[test]
    fn tampered_ciphertext_rejected() {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);
        let bob_spk_bytes: [u8; 32] = bob_spk.secret.to_bytes();

        let bundle = PreKeyBundle {
            identity_key: bob_id.dh_public(),
            identity_key_ed: bob_id.public().verifying,
            signed_prekey: bob_spk.public(),
            signed_prekey_id: bob_spk.id,
            signed_prekey_sig: bob_spk.sign(&bob_id),
            one_time_prekey: None,
            one_time_prekey_id: None,
            pq_spk_public: None,
            pq_spk_sig: None,
        };

        let (alice_sk, header, pq_ct) = x3dh_send(&alice_id, &bundle).unwrap();
        let bob_sk = x3dh_receive(&bob_id, &bob_spk, None, &header, pq_ct.as_deref(), None).unwrap();

        let mut alice = RatchetState::init_alice(&alice_sk, bob_spk.public());
        let mut bob = RatchetState::init_bob(&bob_sk, StaticSecret::from(bob_spk_bytes));

        let ad = b"session";
        let mut msg = alice.encrypt(b"secret", ad);

        // Flip a byte in the ciphertext.
        if let Some(b) = msg.ciphertext.first_mut() {
            *b ^= 0xff;
        }

        assert!(
            matches!(bob.decrypt(&msg, ad), Err(CryptoError::DecryptionFailed)),
            "tampered message must not decrypt"
        );
    }
}
