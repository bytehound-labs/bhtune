-- Observed opcda-bridge gateway compatibility for one live OPC DA run.
--
-- Nullable because simulator and replay runs have no gateway, and a run row is
-- inserted before the compatibility snapshot is recorded. The JSON is the
-- driver's compatibility report (gateway version, overall status, and protocol
-- ranges). It is stored raw so a future report field does not require another
-- migration; readers that cannot parse it treat the snapshot as absent.
ALTER TABLE tune_runs
    ADD COLUMN gateway_compatibility_json TEXT
        CHECK (
            gateway_compatibility_json IS NULL
            OR json_valid(gateway_compatibility_json)
        );
