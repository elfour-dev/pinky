BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS ollama_configuration (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    endpoint TEXT NOT NULL,
    model TEXT NOT NULL,
    configured_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO schema_components(component, version, updated_at)
VALUES ('ollama_configuration', 1, CURRENT_TIMESTAMP)
ON CONFLICT(component) DO UPDATE SET
    version = excluded.version,
    updated_at = excluded.updated_at;

COMMIT;
