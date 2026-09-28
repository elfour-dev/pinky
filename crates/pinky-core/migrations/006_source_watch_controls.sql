BEGIN IMMEDIATE;

ALTER TABLE sources ADD COLUMN watch_paused INTEGER NOT NULL DEFAULT 0 CHECK(watch_paused IN (0, 1));

INSERT INTO schema_components(component, version, updated_at)
VALUES ('source_watch_controls', 1, CURRENT_TIMESTAMP)
ON CONFLICT(component) DO UPDATE SET version = excluded.version, updated_at = excluded.updated_at;

COMMIT;
