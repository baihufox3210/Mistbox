-- Full-text (trigram) index of source files and Markdown notes, for the
-- diagnose / optimise mode: find where an error message string comes from,
-- and which notes mention a rule, symbol or error. Rebuilt from content.
CREATE VIRTUAL TABLE code_text USING fts5(
    node_id UNINDEXED,
    lang UNINDEXED,
    body,
    tokenize = 'trigram'
);
