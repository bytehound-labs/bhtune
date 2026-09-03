-- Anonymous, short-lived ownership for the public simulator-only demo surface.
CREATE TABLE demo_sessions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    token_hash    TEXT NOT NULL UNIQUE,
    created_at    TIMESTAMP NOT NULL,
    last_seen_at  TIMESTAMP NOT NULL,
    expires_at    TIMESTAMP NOT NULL,
    revoked_at    TIMESTAMP,
    CHECK (length(token_hash) = 64 AND token_hash NOT GLOB '*[^0-9a-f]*'),
    CHECK (expires_at > created_at),
    CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);

CREATE INDEX idx_demo_sessions_expires_at ON demo_sessions(expires_at);

ALTER TABLE tune_runs ADD COLUMN demo_session_id INTEGER
    REFERENCES demo_sessions(id) ON DELETE CASCADE;

CREATE INDEX idx_tune_runs_demo_session ON tune_runs(demo_session_id, started_at DESC);
CREATE INDEX idx_tune_runs_demo_session_outcome
    ON tune_runs(demo_session_id, outcome);

CREATE TRIGGER tune_runs_demo_session_insert
BEFORE INSERT ON tune_runs
WHEN NEW.demo_session_id IS NOT NULL AND NEW.driver <> 'simulator'
BEGIN
    SELECT RAISE(ABORT, 'demo_session_id requires simulator driver');
END;

CREATE TRIGGER tune_runs_demo_session_valid_insert
BEFORE INSERT ON tune_runs
WHEN NEW.demo_session_id IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM demo_sessions
    WHERE id = NEW.demo_session_id
      AND revoked_at IS NULL
      AND created_at <= NEW.started_at
      AND expires_at > NEW.started_at
)
BEGIN
    SELECT RAISE(ABORT, 'demo_session_id requires an active demo session');
END;

-- DemoPolicy fixes this cap at 5,000. The repository count supports an early friendly
-- rejection; this trigger is the authoritative race-safe backstop for concurrent starts.
CREATE TRIGGER tune_runs_demo_global_limit_insert
BEFORE INSERT ON tune_runs
WHEN NEW.demo_session_id IS NOT NULL
 AND (SELECT COUNT(*) FROM tune_runs WHERE demo_session_id IS NOT NULL) >= 5000
BEGIN
    SELECT RAISE(ABORT, 'demo tune run row limit reached');
END;

CREATE TRIGGER tune_runs_demo_session_update
BEFORE UPDATE OF driver ON tune_runs
WHEN NEW.demo_session_id IS NOT NULL AND NEW.driver <> 'simulator'
BEGIN
    SELECT RAISE(ABORT, 'demo_session_id requires simulator driver');
END;

-- Ownership must be present on the initial tune_runs insert. It cannot be attached, removed,
-- or transferred afterward.
CREATE TRIGGER tune_runs_demo_session_immutable
BEFORE UPDATE OF demo_session_id ON tune_runs
WHEN NEW.demo_session_id IS NOT OLD.demo_session_id
BEGIN
    SELECT RAISE(ABORT, 'demo_session_id is immutable');
END;
