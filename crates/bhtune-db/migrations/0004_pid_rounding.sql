ALTER TABLE dcs_templates
    ADD COLUMN pid_rounding_kind TEXT NOT NULL DEFAULT 'significant_digits'
    CHECK (pid_rounding_kind IN ('decimal_places', 'significant_digits'));

ALTER TABLE dcs_templates
    ADD COLUMN pid_rounding_digits INTEGER NOT NULL DEFAULT 3
    CHECK (
        typeof(pid_rounding_digits) = 'integer'
        AND pid_rounding_digits BETWEEN 0 AND 7
        AND (pid_rounding_kind = 'decimal_places' OR pid_rounding_digits >= 1)
    );
