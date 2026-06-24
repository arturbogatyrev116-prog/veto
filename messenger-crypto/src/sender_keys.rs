use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit};
use hkdf::Hkdf;
use sha2::Sha256;

/// Encrypt plaintext with a sender chain key at the given counter position.
/// Output: `[4B BE counter][chacha20poly1305 ciphertext+tag]`
pub fn encrypt(chain_key: &[u8; 32], counter: u32, plaintext: &[u8]) -> Vec<u8> {
    let (msg_key, nonce) = derive(chain_key, counter);
    let cipher = ChaCha20Poly1305::new((&msg_key).into());
    let ct = cipher.encrypt((&nonce).into(), plaintext).expect("encrypt");
    let mut out = Vec::with_capacity(4 + ct.len());
    out.extend_from_slice(&counter.to_be_bytes());
    out.extend_from_slice(&ct);
    out
}

/// Decrypt a sender key frame `[4B counter][ciphertext]`.
/// Returns `(plaintext, counter)`.
pub fn decrypt(chain_key: &[u8; 32], frame: &[u8]) -> Result<(Vec<u8>, u32), String> {
    if frame.len() < 4 {
        return Err("frame too short".into());
    }
    let counter = u32::from_be_bytes(frame[..4].try_into().unwrap());
    let (msg_key, nonce) = derive(chain_key, counter);
    let cipher = ChaCha20Poly1305::new((&msg_key).into());
    cipher
        .decrypt((&nonce).into(), &frame[4..])
        .map(|pt| (pt, counter))
        .map_err(|e| e.to_string())
}

/// HKDF-SHA256(ikm=chain_key, salt=counter_bytes, info="sender-chain-v1") → (32B key, 12B nonce)
fn derive(chain_key: &[u8; 32], counter: u32) -> ([u8; 32], [u8; 12]) {
    let hk = Hkdf::<Sha256>::new(Some(&counter.to_be_bytes()), chain_key);
    let mut okm = [0u8; 44];
    hk.expand(b"sender-chain-v1", &mut okm).expect("44 < 8160");
    let mut key = [0u8; 32];
    let mut nonce = [0u8; 12];
    key.copy_from_slice(&okm[..32]);
    nonce.copy_from_slice(&okm[32..44]);
    (key, nonce)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngCore;

    #[test]
    fn round_trip() {
        let mut chain_key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut chain_key);
        let msg = b"hello sender keys";
        let ct = encrypt(&chain_key, 0, msg);
        let (pt, counter) = decrypt(&chain_key, &ct).unwrap();
        assert_eq!(pt, msg);
        assert_eq!(counter, 0);
    }

    #[test]
    fn different_counters_different_keys() {
        let mut chain_key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut chain_key);
        let msg = b"test";
        let ct0 = encrypt(&chain_key, 0, msg);
        let ct1 = encrypt(&chain_key, 1, msg);
        // Counter bytes differ → ciphertexts differ
        assert_ne!(ct0, ct1);
        // Both decrypt correctly
        assert_eq!(decrypt(&chain_key, &ct0).unwrap().0, msg);
        assert_eq!(decrypt(&chain_key, &ct1).unwrap().0, msg);
    }

    #[test]
    fn wrong_key_fails() {
        let mut ck1 = [0u8; 32];
        let mut ck2 = [1u8; 32];
        rand::thread_rng().fill_bytes(&mut ck1);
        rand::thread_rng().fill_bytes(&mut ck2);
        let ct = encrypt(&ck1, 0, b"secret");
        assert!(decrypt(&ck2, &ct).is_err());
    }
}
