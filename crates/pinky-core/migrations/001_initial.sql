BEGIN IMMEDIATE;

CREATE TABLE schema_components (
    component TEXT PRIMARY KEY,
    version INTEGER NOT NULL CHECK(version >= 1),
    updated_at TEXT NOT NULL
);

INSERT INTO schema_components(component, version, updated_at) VALUES
    ('metadata', 1, CURRENT_TIMESTAMP),
    ('object_store', 1, CURRENT_TIMESTAMP),
    ('lexical_index', 1, CURRENT_TIMESTAMP),
    ('vector_index', 1, CURRENT_TIMESTAMP);

CREATE TABLE objects (
    sha256 TEXT PRIMARY KEY CHECK(length(sha256) = 64),
    uncompressed_length INTEGER NOT NULL,
    compressed_length INTEGER NOT NULL,
    mime_type TEXT NOT NULL,
    compression_level INTEGER NOT NULL,
    reference_count INTEGER NOT NULL DEFAULT 0 CHECK(reference_count >= 0),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE sources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('local_file','web_page','search_result','generated')),
    canonical_uri TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    approval_scope TEXT NOT NULL,
    current_version_id TEXT,
    refresh_policy TEXT NOT NULL,
    authority_tier INTEGER NOT NULL CHECK(authority_tier BETWEEN 1 AND 4),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_checked_at TEXT,
    state TEXT NOT NULL CHECK(state IN ('active','missing','blocked','unsupported','deleted'))
);

CREATE TABLE source_versions (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES sources(id),
    original_object_hash TEXT NOT NULL REFERENCES objects(sha256),
    extracted_object_hash TEXT REFERENCES objects(sha256),
    mime_type TEXT NOT NULL,
    detected_language TEXT,
    byte_size INTEGER NOT NULL,
    extraction_method TEXT,
    extraction_version TEXT,
    retrieved_at TEXT NOT NULL,
    http_status INTEGER,
    selected_headers_json TEXT,
    superseded_version_id TEXT REFERENCES source_versions(id),
    processing_state TEXT NOT NULL,
    error TEXT,
    citation_map_json TEXT
);

CREATE TABLE chunks (
    id TEXT PRIMARY KEY,
    source_version_id TEXT NOT NULL REFERENCES source_versions(id),
    ordinal INTEGER NOT NULL,
    heading_path TEXT,
    character_start INTEGER NOT NULL,
    character_end INTEGER NOT NULL,
    byte_start INTEGER NOT NULL,
    byte_end INTEGER NOT NULL,
    coordinates_json TEXT,
    token_count INTEGER NOT NULL,
    extracted_text_hash TEXT NOT NULL REFERENCES objects(sha256),
    embedding_id TEXT,
    UNIQUE(source_version_id, ordinal)
);

CREATE TABLE topics (
    id TEXT PRIMARY KEY,
    label TEXT NOT NULL,
    aliases_json TEXT NOT NULL,
    summary_object_hash TEXT REFERENCES objects(sha256),
    coverage_score REAL NOT NULL DEFAULT 0 CHECK(coverage_score BETWEEN 0 AND 100),
    freshness_status TEXT NOT NULL,
    unresolved_questions_json TEXT NOT NULL,
    last_consolidated_at TEXT
);

CREATE TABLE claims (
    id TEXT PRIMARY KEY,
    subject TEXT NOT NULL,
    predicate TEXT NOT NULL,
    object TEXT NOT NULL,
    topic_id TEXT REFERENCES topics(id),
    status TEXT NOT NULL CHECK(status IN ('supported','disputed','stale','unverified')),
    confidence REAL NOT NULL CHECK(confidence BETWEEN 0 AND 1),
    first_seen_at TEXT NOT NULL,
    last_verified_at TEXT
);

CREATE TABLE claim_evidence (
    claim_id TEXT NOT NULL REFERENCES claims(id),
    chunk_id TEXT NOT NULL REFERENCES chunks(id),
    relationship TEXT NOT NULL CHECK(relationship IN ('supporting','contradicting')),
    PRIMARY KEY(claim_id, chunk_id, relationship)
);

CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    parent_id TEXT REFERENCES tasks(id),
    kind TEXT NOT NULL,
    state TEXT NOT NULL,
    phase TEXT NOT NULL,
    progress REAL,
    permission_scope_json TEXT NOT NULL,
    research_budget_json TEXT NOT NULL,
    cancellation_requested INTEGER NOT NULL DEFAULT 0,
    event_log_hash TEXT REFERENCES objects(sha256),
    error_code TEXT,
    recoverable INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id),
    ordinal INTEGER NOT NULL,
    role TEXT NOT NULL,
    content_object_hash TEXT NOT NULL REFERENCES objects(sha256),
    model TEXT,
    citations_json TEXT NOT NULL,
    task_id TEXT REFERENCES tasks(id),
    replaces_message_id TEXT REFERENCES messages(id),
    created_at TEXT NOT NULL,
    UNIQUE(conversation_id, ordinal)
);

CREATE TABLE task_artifacts (
    task_id TEXT NOT NULL REFERENCES tasks(id),
    object_hash TEXT NOT NULL REFERENCES objects(sha256),
    kind TEXT NOT NULL,
    PRIMARY KEY(task_id, object_hash)
);

CREATE INDEX idx_source_versions_source ON source_versions(source_id, retrieved_at DESC);
CREATE INDEX idx_chunks_version ON chunks(source_version_id, ordinal);
CREATE INDEX idx_claims_topic ON claims(topic_id, status);
CREATE INDEX idx_messages_conversation ON messages(conversation_id, ordinal);
CREATE INDEX idx_tasks_parent ON tasks(parent_id, created_at);

COMMIT;
