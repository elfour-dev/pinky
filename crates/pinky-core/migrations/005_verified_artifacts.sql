BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS verified_artifacts (
    artifact_id TEXT PRIMARY KEY,
    key_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('model', 'executable')),
    capability TEXT NOT NULL,
    version TEXT NOT NULL,
    url TEXT NOT NULL,
    sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
    byte_size INTEGER NOT NULL CHECK(byte_size > 0),
    license_url TEXT NOT NULL,
    runtime_version TEXT NOT NULL,
    context_length INTEGER,
    minimum_ram_bytes INTEGER NOT NULL CHECK(minimum_ram_bytes > 0),
    installed_path TEXT NOT NULL,
    signed_manifest_json TEXT NOT NULL,
    installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO schema_components(component, version, updated_at)
VALUES ('verified_artifacts', 1, CURRENT_TIMESTAMP)
ON CONFLICT(component) DO UPDATE SET
    version = excluded.version,
    updated_at = excluded.updated_at;

COMMIT;
