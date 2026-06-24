//! Wire format: length-prefixed postcard frames.
//!
//! Frame layout (all big-endian):
//!   [4 bytes: payload length u32] [payload: postcard-encoded T]
//!
//! This gives a self-delimiting stream suitable for TCP or WebSocket binary frames.
//! The 4-byte prefix caps frames at 4 GiB; actual messages are orders of magnitude smaller.

use serde::{Deserialize, Serialize};

use crate::error::CryptoError;

// ── Envelope types ─────────────────────────────────────────────────────────────

/// First message Alice sends to Bob: X3DH handshake + first ratchet message.
#[derive(Debug, Serialize, Deserialize)]
pub struct InitEnvelope {
    /// Alice's X3DH header (ik_a, ek_a, spk_id, opk_id).
    pub x3dh: crate::x3dh::X3dhHeader,
    /// The first Double Ratchet message (encrypted with the X3DH shared secret).
    pub message: crate::ratchet::Message,
    /// Conversation AD (application-defined, e.g. SHA-256(ik_alice || ik_bob)).
    pub ad: Vec<u8>,
    /// ML-KEM-768 ciphertext from Alice's PQ encapsulation (1088 bytes).
    /// Present only when Bob's bundle advertised a pq_spk_public.
    pub pq_ct: Option<Vec<u8>>,
}

/// Subsequent messages after the session is established.
#[derive(Debug, Serialize, Deserialize)]
pub struct MessageEnvelope {
    /// Session identifier (server assigns; opaque to crypto layer).
    pub session_id: [u8; 16],
    /// The Double Ratchet ciphertext.
    pub message: crate::ratchet::Message,
    /// Associated data included in AEAD.
    pub ad: Vec<u8>,
}

// ── Encode / Decode ────────────────────────────────────────────────────────────

/// Encode a value to length-prefixed postcard bytes.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, WireError> {
    let payload = postcard::to_stdvec(value).map_err(WireError::Encode)?;
    let len = u32::try_from(payload.len()).map_err(|_| WireError::TooBig(payload.len()))?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decode a length-prefixed postcard frame from a byte slice.
///
/// Returns `(value, bytes_consumed)`.  The caller must ensure the slice contains
/// at least one complete frame; call [`frame_len`] first if unsure.
pub fn decode<T: for<'de> Deserialize<'de>>(buf: &[u8]) -> Result<(T, usize), WireError> {
    if buf.len() < 4 {
        return Err(WireError::Incomplete);
    }
    let payload_len = u32::from_be_bytes(buf[..4].try_into().unwrap()) as usize;
    let total = 4 + payload_len;
    if buf.len() < total {
        return Err(WireError::Incomplete);
    }
    let value = postcard::from_bytes::<T>(&buf[4..total]).map_err(WireError::Decode)?;
    Ok((value, total))
}

/// Return the total frame size (header + payload) without decoding, or `None` if
/// the buffer is shorter than 4 bytes.
pub fn frame_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    let payload_len = u32::from_be_bytes(buf[..4].try_into().unwrap()) as usize;
    Some(4 + payload_len)
}

// ── Error type ─────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("buffer too short — need more bytes")]
    Incomplete,
    #[error("frame too large: {0} bytes")]
    TooBig(usize),
    #[error("postcard encode error: {0}")]
    Encode(postcard::Error),
    #[error("postcard decode error: {0}")]
    Decode(postcard::Error),
}

impl From<WireError> for CryptoError {
    fn from(_: WireError) -> Self {
        CryptoError::DecryptionFailed
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use x25519_dalek::StaticSecret;

    use super::*;
    use crate::{
        keys::{IdentityKeyPair, PreKeyBundle, SignedPreKey},
        ratchet::RatchetState,
        x3dh::{x3dh_receive, x3dh_send},
    };

    fn make_session() -> (RatchetState, RatchetState, crate::x3dh::X3dhHeader) {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);
        let bob_spk_bytes = bob_spk.secret.to_bytes();

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

        let alice = RatchetState::init_alice(&alice_sk, bob_spk.public());
        let bob = RatchetState::init_bob(&bob_sk, StaticSecret::from(bob_spk_bytes));
        (alice, bob, header)
    }

    #[test]
    fn message_roundtrip() {
        let (mut alice, mut bob, _) = make_session();
        let ad = b"session-ad";

        let original = alice.encrypt(b"hello wire format", ad);
        let frame = encode(&original).unwrap();

        let (decoded, consumed): (crate::ratchet::Message, _) = decode(&frame).unwrap();
        assert_eq!(consumed, frame.len());

        let plaintext = bob.decrypt(&decoded, ad).unwrap();
        assert_eq!(plaintext, b"hello wire format");
    }

    #[test]
    fn frame_len_correct() {
        let (mut alice, _, _) = make_session();
        let msg = alice.encrypt(b"test", b"ad");
        let frame = encode(&msg).unwrap();

        assert_eq!(frame_len(&frame), Some(frame.len()));
        assert_eq!(frame_len(&frame[..2]), None);
    }

    #[test]
    fn incomplete_frame_error() {
        let (mut alice, _, _) = make_session();
        let msg = alice.encrypt(b"test", b"ad");
        let frame = encode(&msg).unwrap();

        let result: Result<(crate::ratchet::Message, _), _> = decode(&frame[..frame.len() - 1]);
        assert!(matches!(result, Err(WireError::Incomplete)));
    }

    #[test]
    fn init_envelope_roundtrip() {
        let alice_id = IdentityKeyPair::generate();
        let bob_id = IdentityKeyPair::generate();
        let bob_spk = SignedPreKey::generate(1);
        let bob_spk_bytes = bob_spk.secret.to_bytes();

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

        let (alice_sk, x3dh_header, pq_ct) = x3dh_send(&alice_id, &bundle).unwrap();
        let bob_sk = x3dh_receive(&bob_id, &bob_spk, None, &x3dh_header, pq_ct.as_deref(), None).unwrap();

        let mut alice = RatchetState::init_alice(&alice_sk, bob_spk.public());
        let mut bob = RatchetState::init_bob(&bob_sk, StaticSecret::from(bob_spk_bytes));

        let ad = b"alice-bob-v1";
        let first_msg = alice.encrypt(b"first message", ad);

        let envelope = InitEnvelope {
            x3dh: x3dh_header,
            pq_ct,
            message: first_msg,
            ad: ad.to_vec(),
        };

        let frame = encode(&envelope).unwrap();
        let (decoded, _): (InitEnvelope, _) = decode(&frame).unwrap();

        let plaintext = bob.decrypt(&decoded.message, &decoded.ad).unwrap();
        assert_eq!(plaintext, b"first message");
    }

    #[test]
    fn multi_frame_stream() {
        let (mut alice, mut bob, _) = make_session();
        let ad = b"stream";

        let mut stream = Vec::new();
        let messages: Vec<&[u8]> = vec![b"one", b"two", b"three"];
        for pt in &messages {
            stream.extend_from_slice(&encode(&alice.encrypt(pt, ad)).unwrap());
        }

        let mut pos = 0;
        for expected in &messages {
            let (msg, consumed): (crate::ratchet::Message, _) = decode(&stream[pos..]).unwrap();
            pos += consumed;
            let plaintext = bob.decrypt(&msg, ad).unwrap();
            assert_eq!(&plaintext, expected);
        }
        assert_eq!(pos, stream.len());
    }
}
