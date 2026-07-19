-- Keep recovered capacity unavailable until the enrollment posture and its
-- cluster profile match the evidence accepted by the recovery workflow.

CREATE OR REPLACE FUNCTION server_track_incident_recovery()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    recovery_incident_id UUID;
    recovery_operation_id UUID;
    recovery_target_revision UUID;
    recovery_ready BOOLEAN;
BEGIN
    IF NEW.lifecycle_state <> 'active'
       OR OLD.lifecycle_state = 'active'
       OR NOT (NEW.progress ? 'incident_id')
       OR NOT (NEW.progress ? 'operation_id')
       OR NOT (NEW.progress ? 'fault_cleared_target_revision') THEN
        RETURN NEW;
    END IF;

    recovery_incident_id := (NEW.progress ->> 'incident_id')::UUID;
    recovery_operation_id := (NEW.progress ->> 'operation_id')::UUID;
    recovery_target_revision := (NEW.progress ->> 'fault_cleared_target_revision')::UUID;

    SELECT EXISTS (
        SELECT 1
          FROM server_incidents AS incident
          JOIN server_target_health AS health
            ON health.tenant_id = incident.tenant_id
           AND health.host_id = incident.host_id
          JOIN server_clusters AS cluster
            ON cluster.tenant_id = NEW.tenant_id
           AND cluster.cluster_id = NEW.cluster_id
         WHERE incident.tenant_id = NEW.tenant_id
           AND incident.incident_id = recovery_incident_id
           AND incident.status = 'closed'
           AND health.target_id = NEW.target_id
           AND health.status = 'healthy'
           AND health.revision = recovery_target_revision
           AND cluster.revision = NEW.cluster_revision
           AND NEW.health_status = 'healthy'
           AND NEW.compliance_status = 'compliant'
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
        RAISE EXCEPTION 'incident recovery outcome requires exact fresh healthy evidence'
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
        recovery_operation_id,
        NEW.tenant_id,
        recovery_operation_id::TEXT,
        'server_target',
        NEW.target_id,
        'incident-recovery',
        jsonb_build_object(
            'lifecycle_state', OLD.lifecycle_state,
            'health_status', OLD.health_status,
            'compliance_status', OLD.compliance_status,
            'qualification_status', OLD.qualification_status,
            'capacity', 'isolated'
        ),
        jsonb_build_object(
            'lifecycle_state', NEW.lifecycle_state,
            'health_status', NEW.health_status,
            'compliance_status', NEW.compliance_status,
            'qualification_status', NEW.qualification_status,
            'capacity', 'available',
            'health_revision', recovery_target_revision
        ),
        'succeeded',
        COALESCE(NULLIF(NEW.progress ->> 'transitioned_by', ''), 'server-administrator'),
        recovery_incident_id
    );

    RETURN NEW;
END;
$$;

COMMENT ON FUNCTION server_track_incident_recovery() IS
    'Atomically links current-profile validation and healthy posture to verified incident recovery and returned capacity.';
