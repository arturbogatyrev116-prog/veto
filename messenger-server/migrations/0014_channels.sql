ALTER TABLE groups ADD COLUMN IF NOT EXISTS auto_subscribe_channels BOOLEAN NOT NULL DEFAULT TRUE;

CREATE TABLE IF NOT EXISTS channels (
    channel_id  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    group_id    UUID        NOT NULL REFERENCES groups(group_id) ON DELETE CASCADE,
    name        TEXT        NOT NULL,
    description TEXT,
    created_by  UUID        NOT NULL REFERENCES users(id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(group_id, name)
);

CREATE TABLE IF NOT EXISTS channel_subscriptions (
    channel_id UUID NOT NULL REFERENCES channels(channel_id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users(id)            ON DELETE CASCADE,
    PRIMARY KEY (channel_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_channels_group ON channels(group_id);
