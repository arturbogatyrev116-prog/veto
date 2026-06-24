CREATE TABLE prekey_bundles (
    user_id      UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    bundle_bytes BYTEA NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
