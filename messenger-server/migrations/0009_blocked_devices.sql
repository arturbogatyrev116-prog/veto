ALTER TABLE users
    ADD COLUMN blocked_at     TIMESTAMPTZ,
    ADD COLUMN blocked_reason TEXT;

CREATE TABLE blocked_devices (
    device_id  TEXT PRIMARY KEY,
    blocked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    reason     TEXT
);

CREATE INDEX idx_blocked_devices_at ON blocked_devices(blocked_at);
