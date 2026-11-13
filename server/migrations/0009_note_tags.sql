-- Front-matter tags of Markdown notes (rules / known errors / symbols / error
-- messages / free tags): link notes to the diagnose mode and the problems panel.
CREATE TABLE note_tags (
    node_id TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind    TEXT NOT NULL,
    value   TEXT NOT NULL
);
CREATE INDEX note_tags_node ON note_tags(node_id);
CREATE INDEX note_tags_kv ON note_tags(kind, value);
