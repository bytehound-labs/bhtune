-- Audit every accepted OPC DA manipulated-variable command independently of the
-- commanded-MV sample trail. A row is inserted as pending immediately after the
-- write is accepted, then updated with the latest live readback and a terminal
-- status once the command is confirmed, rejected, abandoned, or deliberately
-- handed off to the restore path.
CREATE TABLE tune_mv_actuations (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id                   INTEGER NOT NULL REFERENCES tune_runs(id) ON DELETE CASCADE,
    sequence                 INTEGER NOT NULL CHECK (sequence >= 0),
    kind                     TEXT NOT NULL CHECK (kind IN ('relay', 'restore')),
    commanded_at             TEXT NOT NULL,
    target_mv                REAL NOT NULL,
    previous_commanded_mv    REAL,
    tolerance                REAL NOT NULL CHECK (tolerance >= 0),
    confirmation_due_at      TEXT NOT NULL,
    last_checked_at          TEXT,
    readback_mv              REAL,
    readback_quality         TEXT CHECK (
        readback_quality IS NULL
        OR readback_quality IN ('good', 'uncertain', 'bad')
    ),
    attempt_count            INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    status                   TEXT NOT NULL DEFAULT 'pending' CHECK (
        status IN ('pending', 'confirmed', 'failed', 'unverified', 'superseded')
    ),
    detail                   TEXT,
    UNIQUE (run_id, sequence)
);
