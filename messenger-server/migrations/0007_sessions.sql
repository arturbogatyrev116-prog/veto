ALTER TABLE auth_tokens
    ADD COLUMN session_id   UUID        NOT NULL DEFAULT gen_random_uuid(),
    ADD COLUMN device_name  TEXT        NOT NULL DEFAULT 'Unknown Device',
    ADD COLUMN device_id    TEXT        NOT NULL DEFAULT gen_random_uuid()::text,
    ADD COLUMN last_seen    TIMESTAMPTZ NOT NULL DEFAULT now();

CREATE UNIQUE INDEX idx_auth_tokens_session_id ON auth_tokens(session_id);
CREATE INDEX idx_auth_tokens_device_id ON auth_tokens(device_id);
