-- Role hierarchy: owner > admin > moderator > member
-- Only one owner per group (the creator). Transfer via transfer_ownership endpoint.
ALTER TABLE group_members
    ADD COLUMN role TEXT NOT NULL DEFAULT 'member'
        CHECK (role IN ('owner', 'admin', 'moderator', 'member'));
