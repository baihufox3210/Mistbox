-- Code index: symbols, call sites, static-analysis findings and clone
-- fingerprints for source files (see src/code). Rebuilt from file content, so
-- it can always be dropped and recomputed.

-- One row per file the indexer has looked at (keyed to content + engine version).
CREATE TABLE code_files (
    node_id    TEXT    PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    sha256     TEXT    NOT NULL,
    engine     INTEGER NOT NULL,
    lang       TEXT,
    status     TEXT    NOT NULL CHECK (status IN ('indexed', 'skipped', 'failed')),
    detail     TEXT,
    lines      INTEGER NOT NULL DEFAULT 0,
    tokens     INTEGER NOT NULL DEFAULT 0,
    indexed_at INTEGER NOT NULL
);

CREATE TABLE code_symbols (
    node_id    TEXT    NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    kind       TEXT    NOT NULL,
    line_start INTEGER NOT NULL,
    line_end   INTEGER NOT NULL
);
CREATE INDEX code_symbols_node ON code_symbols(node_id);
CREATE INDEX code_symbols_name ON code_symbols(name);

CREATE TABLE code_refs (
    node_id   TEXT    NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    name      TEXT    NOT NULL,
    line      INTEGER NOT NULL,
    in_symbol TEXT
);
CREATE INDEX code_refs_node ON code_refs(node_id);
CREATE INDEX code_refs_name ON code_refs(name);

CREATE TABLE code_findings (
    id        INTEGER PRIMARY KEY,
    node_id   TEXT    NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    rule      TEXT    NOT NULL,
    severity  TEXT    NOT NULL,
    line      INTEGER NOT NULL,
    col       INTEGER NOT NULL,
    line_end  INTEGER NOT NULL,
    message   TEXT    NOT NULL,
    symbol    TEXT
);
CREATE INDEX code_findings_node ON code_findings(node_id);

CREATE TABLE code_fp (
    hash       INTEGER NOT NULL,
    node_id    TEXT    NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    tok        INTEGER NOT NULL,
    line_start INTEGER NOT NULL,
    line_end   INTEGER NOT NULL
);
CREATE INDEX code_fp_hash ON code_fp(hash);
CREATE INDEX code_fp_node ON code_fp(node_id);
