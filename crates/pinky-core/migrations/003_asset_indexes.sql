BEGIN IMMEDIATE;

CREATE INDEX IF NOT EXISTS idx_sources_asset_updated
    ON sources (state, kind, updated_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_sources_asset_name
    ON sources (display_name COLLATE NOCASE, id);

CREATE INDEX IF NOT EXISTS idx_source_versions_asset_type
    ON source_versions (mime_type, byte_size DESC, id);

INSERT INTO schema_components(component, version, updated_at)
VALUES ('asset_view', 1, CURRENT_TIMESTAMP)
ON CONFLICT(component) DO UPDATE SET
    version = excluded.version,
    updated_at = excluded.updated_at;

COMMIT;
