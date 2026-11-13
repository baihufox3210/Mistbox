-- UI settings (single admin) and RAG index.

CREATE TABLE settings (
    key        TEXT    PRIMARY KEY,
    value      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
);

-- One row per file the indexer has looked at (indexed, skipped or failed),
-- keyed to the content hash so edits / re-uploads trigger re-indexing.
CREATE TABLE rag_docs (
    node_id    TEXT    PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    sha256     TEXT    NOT NULL,
    model      TEXT    NOT NULL,
    status     TEXT    NOT NULL CHECK (status IN ('indexed', 'skipped', 'failed')),
    detail     TEXT,
    chunks     INTEGER NOT NULL DEFAULT 0,
    indexed_at INTEGER NOT NULL
);

CREATE TABLE rag_chunks (
    id         INTEGER PRIMARY KEY,
    node_id    TEXT    NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    line_start INTEGER NOT NULL,
    line_end   INTEGER NOT NULL,
    text       TEXT    NOT NULL,
    embedding  BLOB    NOT NULL          -- little-endian f32, L2-normalised
);
CREATE INDEX rag_chunks_node ON rag_chunks(node_id);
