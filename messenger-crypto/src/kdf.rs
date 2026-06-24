use hkdf::Hkdf;
use sha2::Sha256;

/// HKDF-SHA256 with explicit info tag.
pub fn hkdf_sha256(ikm: &[u8], salt: Option<&[u8]>, info: &[u8], out: &mut [u8]) {
    let hk = Hkdf::<Sha256>::new(salt, ikm);
    hk.expand(info, out).expect("HKDF output length is valid");
}

/// Derive a 32-byte key with a typed info label.
pub fn derive_key(ikm: &[u8], salt: Option<&[u8]>, info: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    hkdf_sha256(ikm, salt, info, &mut out);
    out
}

/// KDF_RK: Root Key ratchet step.
/// Returns (new_root_key, chain_key).
pub fn kdf_rk(root_key: &[u8; 32], dh_out: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let mut out = [0u8; 64];
    hkdf_sha256(dh_out, Some(root_key), b"messenger-ratchet-v1", &mut out);
    let mut rk = [0u8; 32];
    let mut ck = [0u8; 32];
    rk.copy_from_slice(&out[..32]);
    ck.copy_from_slice(&out[32..]);
    (rk, ck)
}

/// KDF_CK: Chain Key step.
/// Returns (next_chain_key, message_key).
pub fn kdf_ck(chain_key: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let next_ck = derive_key(chain_key, None, b"messenger-chain-v1");
    let msg_key = derive_key(chain_key, None, b"messenger-msgkey-v1");
    (next_ck, msg_key)
}

/// Derive the three keys needed for AEAD encryption from a message key.
/// Returns (encryption_key, auth_key, iv).
pub fn message_keys(mk: &[u8; 32]) -> ([u8; 32], [u8; 32], [u8; 12]) {
    let mut out = [0u8; 76];
    hkdf_sha256(mk, None, b"messenger-msgaead-v1", &mut out);
    let mut enc = [0u8; 32];
    let mut auth = [0u8; 32];
    let mut iv = [0u8; 12];
    enc.copy_from_slice(&out[..32]);
    auth.copy_from_slice(&out[32..64]);
    iv.copy_from_slice(&out[64..76]);
    (enc, auth, iv)
}
