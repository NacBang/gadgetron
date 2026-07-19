-- Preserve only the Core-issued pointer that proves an operation outcome was
-- returned to the experience lifecycle. Core remains the feedback authority.

ALTER TABLE server_operation_outcomes
    ADD COLUMN experience_revision TEXT CHECK (
        experience_revision IS NULL
        OR length(experience_revision) BETWEEN 1 AND 256
    );

CREATE INDEX server_operation_outcomes_experience_idx
    ON server_operation_outcomes (tenant_id, experience_revision)
    WHERE experience_revision IS NOT NULL;

ALTER TABLE server_incident_events
    DROP CONSTRAINT server_incident_events_event_kind_check;

ALTER TABLE server_incident_events
    ADD CONSTRAINT server_incident_events_event_kind_check CHECK (
        event_kind IN (
            'opened',
            'state_changed',
            'closed',
            'action_succeeded',
            'action_failed',
            'action_indeterminate',
            'experience_recorded'
        )
    );

CREATE OR REPLACE FUNCTION server_track_incident_experience()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.incident_id IS NULL
       OR NEW.experience_revision IS NULL
       OR NEW.experience_revision IS NOT DISTINCT FROM OLD.experience_revision THEN
        RETURN NEW;
    END IF;

    INSERT INTO server_incident_events (
        tenant_id,
        incident_id,
        event_kind,
        occurred_at,
        summary,
        details
    ) VALUES (
        NEW.tenant_id,
        NEW.incident_id,
        'experience_recorded',
        now(),
        'Outcome returned to learning',
        jsonb_build_object(
            'operation_id', NEW.operation_id,
            'experience_revision', NEW.experience_revision
        )
    );

    UPDATE server_incidents
       SET revision = gen_random_uuid()
     WHERE tenant_id = NEW.tenant_id
       AND incident_id = NEW.incident_id;

    RETURN NEW;
END;
$$;

CREATE TRIGGER server_incident_experience_timeline
AFTER UPDATE OF experience_revision ON server_operation_outcomes
FOR EACH ROW EXECUTE FUNCTION server_track_incident_experience();

COMMENT ON COLUMN server_operation_outcomes.experience_revision IS
    'Opaque Core OutcomeFeedback receipt revision; this pointer does not imply that a Lesson was created or reviewed.';
