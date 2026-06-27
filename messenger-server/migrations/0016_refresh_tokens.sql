-- Short-lived access tokens (24 h) + long-lived refresh tokens (30 d).
-- expires_at IS NULL means the token never expires (backward-compat for existing rows).
ALTER TABLE auth_tokens
    ADD COLUMN expires_at          TIMESTAMPTZ,
    ADD COLUMN refresh_token_hash  BYTEA,
    ADD COLUMN refresh_expires_at  TIMESTAMPTZ;

CREATE INDEX idx_auth_tokens_refresh
    ON auth_tokens(refresh_token_hash)
    WHERE refresh_token_hash IS NOT NULL;
