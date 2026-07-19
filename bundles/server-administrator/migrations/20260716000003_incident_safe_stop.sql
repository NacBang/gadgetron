-- Record incident-scoped removal from usable cluster capacity in the same
-- transaction as the enrollment transition. The Bundle validates the request
-- first; this trigger keeps the database invariant authoritative.

CREATE OR REPLACE FUNCTION server_track_incident_safe_stop()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    safe_stop_incident_id UUID;
    safe_stop_operation_id UUID;
    incident_matches_target BOOLEAN;
BEGIN
    IF NEW.lifecycle_state <> 'quarantined'
       OR OLD.lifecycle_state = 'quarantined'
       OR NOT (NEW.progress ? 'incident_id')
       OR NOT (NEW.progress ? 'operation_id') THEN
        RETURN NEW;
    END IF;

    safe_stop_incident_id := (NEW.progress ->> 'incident_id')::UUID;
    safe_stop_operation_id := (NEW.progress ->> 'operation_id')::UUID;

    SELECT EXISTS (
        SELECT 1
          FROM server_incidents AS incident
          JOIN server_target_health AS health
            ON health.tenant_id = incident.tenant_id
           AND health.host_id = incident.host_id
         WHERE incident.tenant_id = NEW.tenant_id
           AND incident.incident_id = safe_stop_incident_id
           AND incident.status = 'active'
           AND incident.severity = 'critical'
           AND health.target_id = NEW.target_id
           AND EXISTS (
               SELECT 1
                 FROM server_incident_signals AS signal
                WHERE signal.tenant_id = incident.tenant_id
                  AND signal.incident_id = incident.incident_id
                  AND signal.ended_at IS NULL
                  AND signal.source_state = 'firing'
                  AND signal.severity = 'critical'
           )
    ) INTO incident_matches_target;

    IF NOT incident_matches_target THEN
        RAISE EXCEPTION 'incident safe stop requires an active critical firing signal for the exact server'
            USING ERRCODE = '23514';
    END IF;

    INSERT INTO server_operation_outcomes (
        id,
        tenant_id,
        operation_id,
        target_kind,
        target_id,
        action,
        before_state,
        after_state,
        observed_outcome,
        actor_ref,
        incident_id
    ) VALUES (
        safe_stop_operation_id,
        NEW.tenant_id,
        safe_stop_operation_id::TEXT,
        'server_target',
        NEW.target_id,
        'incident-safe-stop',
        jsonb_build_object(
            'lifecycle_state', OLD.lifecycle_state,
            'health_status', OLD.health_status,
            'compliance_status', OLD.compliance_status,
            'qualification_status', OLD.qualification_status
        ),
        jsonb_build_object(
            'lifecycle_state', NEW.lifecycle_state,
            'health_status', NEW.health_status,
            'compliance_status', NEW.compliance_status,
            'qualification_status', NEW.qualification_status,
            'capacity', 'isolated',
            'reason', NEW.last_error ->> 'message'
        ),
        'succeeded',
        COALESCE(NULLIF(NEW.progress ->> 'transitioned_by', ''), 'server-administrator'),
        safe_stop_incident_id
    );

    RETURN NEW;
END;
$$;

CREATE TRIGGER server_enrollment_incident_safe_stop
AFTER UPDATE OF lifecycle_state, progress ON server_enrollments
FOR EACH ROW EXECUTE FUNCTION server_track_incident_safe_stop();

COMMENT ON FUNCTION server_track_incident_safe_stop() IS
    'Atomically links exact active critical incident evidence to enrollment quarantine and its verified outcome.';
