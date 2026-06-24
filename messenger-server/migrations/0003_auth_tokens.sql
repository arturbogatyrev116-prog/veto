-- Tokens are stored as SHA-256(raw_token) so the server never holds the plaintext.
-- Clients receive the raw token once at registration and never again.
CREATE TABLE auth_tokens (
    token_hash  BYTEA PRIMARY KEY,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX auth_tokens_user_id_idx ON auth_tokens(user_id);
