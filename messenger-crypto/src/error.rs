use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("decryption failed (bad key or corrupted ciphertext)")]
    DecryptionFailed,

    #[error("invalid signature")]
    InvalidSignature,

    #[error("no one-time prekey available")]
    NoOneTimePrekey,

    #[error("missing message keys for out-of-order message (chain={chain}, idx={idx})")]
    MissingMessageKey { chain: u32, idx: u32 },

    #[error("message index too far ahead (max skipped={max})")]
    TooManySkippedMessages { max: u32 },

    #[error("post-quantum crypto error")]
    Pq,

    #[error("no PQ decapsulation key (peer must rotate their prekey bundle)")]
    NoPqDecapKey,
}
