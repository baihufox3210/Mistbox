-- Code embeddings for similar-code search (diagnose / optimise mode).
-- Code embeddings (similar-code search): one row per embedded file, and one
-- int8 vector per chunk (function / window). Rebuilt from content.
CREATE TABLE code_vec_docs (
    node_id    TEXT    PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    sha256     TEXT    NOT NULL,
    model      TEXT    NOT NULL,
    chunks     INTEGER NOT NULL,
    indexed_at INTEGER NOT NULL
);

CREATE TABLE code_vecs (
    id         INTEGER PRIMARY KEY,
    node_id    TEXT    NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    line_start INTEGER NOT NULL,
    line_end   INTEGER NOT NULL,
    symbol     TEXT,
    vec        BLOB    NOT NULL           -- int8 × dims, L2-normalised × 127
);
CREATE INDEX code_vecs_node ON code_vecs(node_id);
