BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS hybrid_configuration (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    qdrant_executable TEXT NOT NULL,
    embedding_endpoint TEXT NOT NULL,
    embedding_model TEXT NOT NULL,
    configured_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO schema_components(component, version, updated_at)
VALUES ('hybrid_configuration', 1, CURRENT_TIMESTAMP)
ON CONFLICT(component) DO UPDATE SET
    version = excluded.version,
    updated_at = excluded.updated_at;

COMMIT;
