CREATE TABLE file_store (
    file_id     UUID    PRIMARY KEY DEFAULT gen_random_uuid(),
    uploader_id UUID    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    data        BYTEA   NOT NULL,
    size_bytes  BIGINT  NOT NULL,
    hash        BYTEA   NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_files_uploader ON file_store(uploader_id);
