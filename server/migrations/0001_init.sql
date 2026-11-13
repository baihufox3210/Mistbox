-- Mistbox initial schema. All timestamps are Unix seconds (UTC).

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT    NOT NULL UNIQUE,
    password_hash TEXT    NOT NULL,          -- Argon2id PHC string
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

CREATE TABLE sessions (
    token_hash   TEXT    PRIMARY KEY,        -- SHA-256 of the cookie token; raw token never stored
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    csrf_token   TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL
);

-- Logical file tree. Physical bytes live in blobs/<xx>/<blob_id>; user-supplied
-- names are never used to build host paths.
CREATE TABLE nodes (
    id          TEXT    PRIMARY KEY,         -- random UUID ('root' for the drive root)
    parent_id   TEXT    REFERENCES nodes(id) ON DELETE CASCADE,
    name        TEXT    NOT NULL,
    kind        TEXT    NOT NULL CHECK (kind IN ('dir', 'file')),
    size        INTEGER NOT NULL DEFAULT 0,
    mime        TEXT,
    sha256      TEXT,
    blob_id     TEXT    UNIQUE,
    sensitive   INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    CHECK ((kind = 'file') = (blob_id IS NOT NULL)),
    CHECK ((parent_id IS NULL) = (id = 'root'))
);
CREATE UNIQUE INDEX nodes_parent_name ON nodes(parent_id, name);
CREATE INDEX nodes_name ON nodes(name);

INSERT INTO nodes (id, parent_id, name, kind, created_at, modified_at)
VALUES ('root', NULL, '', 'dir', CAST(strftime('%s', 'now') AS INTEGER), CAST(strftime('%s', 'now') AS INTEGER));

CREATE TABLE api_keys (
    id              TEXT    PRIMARY KEY,     -- public identifier embedded in the key string
    name            TEXT    NOT NULL,
    secret_hash     TEXT    NOT NULL,        -- SHA-256 of the 256-bit secret part
    can_list        INTEGER NOT NULL,
    can_read        INTEGER NOT NULL,
    can_write       INTEGER NOT NULL,
    allow_sensitive INTEGER NOT NULL DEFAULT 0,
    scope_node_id   TEXT    NOT NULL,        -- file or folder; folders cover all descendants
    created_at      INTEGER NOT NULL,
    expires_at      INTEGER,
    last_used_at    INTEGER,
    revoked_at      INTEGER
);

-- Blobs whose nodes were deleted; unlinked by the background GC unless an
-- online backup is in progress.
CREATE TABLE blob_gc (
    blob_id   TEXT    PRIMARY KEY,
    queued_at INTEGER NOT NULL
);

CREATE TABLE maintenance (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    backup_until INTEGER NOT NULL DEFAULT 0
);
INSERT INTO maintenance (id, backup_until) VALUES (1, 0);
