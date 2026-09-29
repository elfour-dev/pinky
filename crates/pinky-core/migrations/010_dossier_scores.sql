BEGIN IMMEDIATE;

-- R10 dossier measures are durable audit data, not transient UI calculations.
ALTER TABLE topics ADD COLUMN authority_score REAL NOT NULL DEFAULT 0 CHECK(authority_score BETWEEN 0 AND 100);
ALTER TABLE topics ADD COLUMN independence_score REAL NOT NULL DEFAULT 0 CHECK(independence_score BETWEEN 0 AND 100);
ALTER TABLE topics ADD COLUMN freshness_score REAL NOT NULL DEFAULT 0 CHECK(freshness_score BETWEEN 0 AND 100);
ALTER TABLE topics ADD COLUMN unresolved_question_score REAL NOT NULL DEFAULT 0 CHECK(unresolved_question_score BETWEEN 0 AND 100);

COMMIT;
