CREATE TABLE key_log (
    id          BIGSERIAL PRIMARY KEY,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key_type    TEXT NOT NULL,
    key_hash    BYTEA NOT NULL,
    prev_hash   BYTEA,
    entry_hash  BYTEA NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_key_log_user ON key_log(user_id, id);
