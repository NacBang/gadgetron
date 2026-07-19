-- log_findings fingerprint roll-up.
--
-- Open findings are unique by `(tenant, host, source, fingerprint)`.
-- Repeated lines update count/ts_last/excerpt on the existing row instead
-- of creating screen-filling duplicate cards.

ALTER TABLE log_findings
    ADD COLUMN IF NOT EXISTS fingerprint TEXT;

UPDATE log_findings
   SET fingerprint = category
 WHERE fingerprint IS NULL OR btrim(fingerprint) = '';

ALTER TABLE log_findings
    ALTER COLUMN fingerprint SET NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS log_findings_open_fingerprint_idx
    ON log_findings(tenant_id, host_id, source, fingerprint)
    WHERE dismissed_at IS NULL;
