-- Recycle bin. Trashed items are moved under a hidden system folder whose
-- name contains a control character, which user-supplied names can never
-- contain (validate_name), so it can't collide or be addressed by path.
INSERT INTO nodes (id, parent_id, name, kind, created_at, modified_at)
VALUES ('trash', 'root', char(1) || 'trash', 'dir', CAST(strftime('%s', 'now') AS INTEGER), CAST(strftime('%s', 'now') AS INTEGER));

CREATE TABLE trash (
    node_id            TEXT    PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    original_parent_id TEXT    NOT NULL,
    original_name      TEXT    NOT NULL,
    original_path      TEXT    NOT NULL,
    trashed_at         INTEGER NOT NULL
);
