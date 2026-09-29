BEGIN IMMEDIATE;

-- R10 keeps extracted structure separate from source prose.  Every relation
-- remains reachable through claim_evidence; these tables only normalize names
-- and make dossiers queryable.
CREATE TABLE IF NOT EXISTS entities (
    id TEXT PRIMARY KEY,
    canonical_name TEXT NOT NULL UNIQUE,
    entity_type TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS entity_aliases (
    entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    alias TEXT NOT NULL,
    normalized_alias TEXT NOT NULL UNIQUE,
    PRIMARY KEY(entity_id, alias)
);

CREATE TABLE IF NOT EXISTS entity_relationships (
    id TEXT PRIMARY KEY,
    subject_entity_id TEXT NOT NULL REFERENCES entities(id),
    predicate TEXT NOT NULL,
    object_entity_id TEXT NOT NULL REFERENCES entities(id),
    claim_id TEXT NOT NULL UNIQUE REFERENCES claims(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    CHECK(subject_entity_id != object_entity_id)
);

CREATE INDEX IF NOT EXISTS idx_entity_relationships_subject ON entity_relationships(subject_entity_id);
CREATE INDEX IF NOT EXISTS idx_entity_relationships_object ON entity_relationships(object_entity_id);
CREATE INDEX IF NOT EXISTS idx_entity_aliases_entity ON entity_aliases(entity_id);
CREATE INDEX IF NOT EXISTS idx_sources_refresh_due ON sources(refresh_policy, last_checked_at, state);

COMMIT;
