-- Allow a completed run to retain an explicitly invalid response-level result without
-- persisting NaN or infinity as if it were a usable tuning value.
--
-- SQLite cannot alter NOT NULL constraints in place, so rebuild the small result table while
-- preserving every existing row and its primary key.
CREATE TABLE tune_results_new (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id             INTEGER NOT NULL REFERENCES tune_runs(id) ON DELETE CASCADE,
    response_level     TEXT NOT NULL CHECK (response_level IN ('aggressive', 'moderate', 'sluggish')),

    kp                 REAL,
    ti_minutes         REAL,
    td_minutes         REAL,
    proportional       REAL,
    integral           REAL,
    derivative         REAL,

    status             TEXT NOT NULL DEFAULT 'valid' CHECK (status IN ('valid', 'invalid')),
    invalid_reason     TEXT CHECK (
        invalid_reason IS NULL OR invalid_reason IN (
            'non_finite_pv_amplitude',
            'non_positive_pv_amplitude',
            'non_finite_period',
            'non_positive_period',
            'non_finite_frequency',
            'non_positive_frequency',
            'non_finite_kp',
            'non_finite_ti_minutes',
            'non_finite_td_minutes',
            'non_finite_proportional',
            'non_finite_integral',
            'non_finite_derivative'
        )
    ),

    CHECK (
        (status = 'valid'
            AND invalid_reason IS NULL
            AND kp IS NOT NULL
            AND ti_minutes IS NOT NULL
            AND td_minutes IS NOT NULL
            AND proportional IS NOT NULL
            AND integral IS NOT NULL
            AND derivative IS NOT NULL)
        OR
        (status = 'invalid'
            AND invalid_reason IS NOT NULL
            AND kp IS NULL
            AND ti_minutes IS NULL
            AND td_minutes IS NULL
            AND proportional IS NULL
            AND integral IS NULL
            AND derivative IS NULL)
    ),
    CHECK (kp IS NULL OR kp BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38),
    CHECK (ti_minutes IS NULL OR ti_minutes BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38),
    CHECK (td_minutes IS NULL OR td_minutes BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38),
    CHECK (proportional IS NULL OR proportional BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38),
    CHECK (integral IS NULL OR integral BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38),
    CHECK (derivative IS NULL OR derivative BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38),

    UNIQUE (run_id, response_level)
);

INSERT INTO tune_results_new (
    id, run_id, response_level, kp, ti_minutes, td_minutes,
    proportional, integral, derivative, status, invalid_reason
)
WITH classified AS (
    SELECT
        *,
        CASE
            WHEN kp IS NULL OR kp NOT BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38
                THEN 'non_finite_kp'
            WHEN ti_minutes IS NULL OR ti_minutes NOT BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38
                THEN 'non_finite_ti_minutes'
            WHEN td_minutes IS NULL OR td_minutes NOT BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38
                THEN 'non_finite_td_minutes'
            WHEN proportional IS NULL OR proportional NOT BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38
                THEN 'non_finite_proportional'
            WHEN integral IS NULL OR integral NOT BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38
                THEN 'non_finite_integral'
            WHEN derivative IS NULL OR derivative NOT BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38
                THEN 'non_finite_derivative'
            ELSE NULL
        END AS legacy_invalid_reason
    FROM tune_results
)
SELECT
    id, run_id, response_level,
    CASE WHEN legacy_invalid_reason IS NULL THEN kp END,
    CASE WHEN legacy_invalid_reason IS NULL THEN ti_minutes END,
    CASE WHEN legacy_invalid_reason IS NULL THEN td_minutes END,
    CASE WHEN legacy_invalid_reason IS NULL THEN proportional END,
    CASE WHEN legacy_invalid_reason IS NULL THEN integral END,
    CASE WHEN legacy_invalid_reason IS NULL THEN derivative END,
    CASE WHEN legacy_invalid_reason IS NULL THEN 'valid' ELSE 'invalid' END,
    legacy_invalid_reason
FROM classified;

DROP TABLE tune_results;
ALTER TABLE tune_results_new RENAME TO tune_results;
CREATE INDEX idx_tune_results_run ON tune_results(run_id);
