ALTER TABLE server_enrollments
    ADD COLUMN IF NOT EXISTS validation_cycle_started_at TIMESTAMPTZ NOT NULL DEFAULT now();

UPDATE server_enrollments AS enrollment
   SET progress = enrollment.progress || jsonb_build_object(
       'isolation_target_revision', health.revision
   )
  FROM server_target_health AS health
 WHERE enrollment.tenant_id = health.tenant_id
   AND enrollment.target_id = health.target_id
   AND enrollment.lifecycle_state = 'quarantined'
   AND enrollment.progress ? 'incident_id'
   AND NOT (enrollment.progress ? 'isolation_target_revision');

COMMENT ON COLUMN server_enrollments.validation_cycle_started_at IS
    'Start of the current commissioning or qualification cycle; older validation results remain history and cannot satisfy the active gate.';

CREATE OR REPLACE FUNCTION server_guard_incident_recovery()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    recovery_incident_id UUID;
    isolation_revision TEXT;
    recovery_ready BOOLEAN;
BEGIN
    IF NOT (OLD.progress ? 'incident_id')
       OR NOT (
           (OLD.lifecycle_state = 'quarantined' AND NEW.lifecycle_state = 'commissioning')
           OR NEW.lifecycle_state = 'active'
       ) THEN
        RETURN NEW;
    END IF;

    recovery_incident_id := (OLD.progress ->> 'incident_id')::UUID;
    isolation_revision := OLD.progress ->> 'isolation_target_revision';

    SELECT EXISTS (
        SELECT 1
          FROM server_incidents AS incident
          JOIN server_target_health AS health
            ON health.tenant_id = incident.tenant_id
           AND health.host_id = incident.host_id
         WHERE incident.tenant_id = NEW.tenant_id
           AND incident.incident_id = recovery_incident_id
           AND incident.status = 'closed'
           AND health.target_id = NEW.target_id
           AND health.status = 'healthy'
           AND isolation_revision IS NOT NULL
           AND health.revision::TEXT <> isolation_revision
           AND NOT EXISTS (
               SELECT 1
                 FROM server_incident_signals AS signal
                WHERE signal.tenant_id = incident.tenant_id
                  AND signal.host_id = incident.host_id
                  AND signal.ended_at IS NULL
                  AND signal.source_state = 'firing'
                  AND signal.severity = 'critical'
           )
    ) INTO recovery_ready;

    IF NOT recovery_ready THEN
        RAISE EXCEPTION 'incident recovery requires a closed condition and a fresh healthy signed snapshot'
            USING ERRCODE = '23514';
    END IF;

    IF OLD.lifecycle_state = 'quarantined'
       AND NEW.lifecycle_state = 'commissioning'
       AND NOT (
           NEW.validation_cycle_started_at > OLD.validation_cycle_started_at
           AND NEW.commissioning_status IN ('pending', 'not_configured')
           AND NEW.qualification_status IN ('pending', 'not_configured')
       ) THEN
        RAISE EXCEPTION 'incident recovery must start a fresh validation cycle'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.lifecycle_state = 'active'
       AND EXISTS (
           SELECT 1
             FROM jsonb_array_elements_text(NEW.required_qualification) AS required(check_id)
            WHERE COALESCE((
                SELECT result.status
                  FROM server_validation_results AS result
                 WHERE result.tenant_id = NEW.tenant_id
                   AND result.enrollment_id = NEW.enrollment_id
                   AND result.gate = 'qualification'
                   AND result.check_id = required.check_id
                   AND result.observed_at >= NEW.validation_cycle_started_at
                 ORDER BY result.observed_at DESC
                 LIMIT 1
            ), '') NOT IN ('pass', 'warning', 'not_applicable')
       ) THEN
        RAISE EXCEPTION 'incident recovery requires current-cycle qualification evidence'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS server_enrollment_incident_recovery_guard ON server_enrollments;
CREATE TRIGGER server_enrollment_incident_recovery_guard
BEFORE UPDATE OF lifecycle_state, progress, validation_cycle_started_at ON server_enrollments
FOR EACH ROW EXECUTE FUNCTION server_guard_incident_recovery();

COMMENT ON FUNCTION server_guard_incident_recovery() IS
    'Prevents an incident-isolated server from reusing stale health or validation evidence during cluster re-entry.';
