-- Share links: anyone holding the link can view (and optionally edit) one file
-- or folder subtree. Only a SHA-256 of the secret is stored; the full link is
-- shown once, at creation.
CREATE TABLE shares (
    id           TEXT    PRIMARY KEY,
    node_id      TEXT    NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    secret_hash  TEXT    NOT NULL,
    can_write    INTEGER NOT NULL DEFAULT 0,
    label        TEXT    NOT NULL DEFAULT '',
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER,
    last_used_at INTEGER,
    use_count    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX shares_node ON shares(node_id);
