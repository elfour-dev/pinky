BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS source_refresh_schedule (
    source_id TEXT PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
    next_due_at TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('scheduled','claimed','retry')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
    last_error TEXT,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_source_refresh_schedule_due ON source_refresh_schedule(state, next_due_at);

COMMIT;
