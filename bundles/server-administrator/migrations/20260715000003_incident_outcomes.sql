-- Link only explicitly incident-scoped operation outcomes to a durable episode.
-- Generic server actions remain unlinked rather than relying on temporal guesses.

ALTER TABLE server_operation_outcomes
    ADD COLUMN incident_id UUID;

ALTER TABLE server_operation_outcomes
    ADD CONSTRAINT server_operation_outcomes_incident_fk
    FOREIGN KEY (tenant_id, incident_id)
    REFERENCES server_incidents (tenant_id, incident_id);

CREATE INDEX server_operation_outcomes_incident_idx
    ON server_operation_outcomes (tenant_id, incident_id, created_at DESC)
    WHERE incident_id IS NOT NULL;

ALTER TABLE server_incident_events
    DROP CONSTRAINT IF EXISTS server_incident_events_event_kind_check;

ALTER TABLE server_incident_events
    ADD CONSTRAINT server_incident_events_event_kind_check CHECK (
        event_kind IN (
            'opened',
            'state_changed',
            'closed',
            'action_succeeded',
            'action_failed',
            'action_indeterminate'
        )
    );

CREATE OR REPLACE FUNCTION server_track_incident_outcome()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    outcome_event_kind TEXT;
BEGIN
    IF NEW.incident_id IS NULL THEN
        RETURN NEW;
    END IF;

    outcome_event_kind := 'action_' || NEW.observed_outcome;
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
        outcome_event_kind,
        NEW.created_at,
        initcap(replace(NEW.action, '-', ' ')) || ' ' || NEW.observed_outcome,
        jsonb_build_object(
            'operation_id', NEW.operation_id,
            'action', NEW.action,
            'outcome', NEW.observed_outcome
        )
    );

    UPDATE server_incidents
       SET revision = gen_random_uuid()
     WHERE tenant_id = NEW.tenant_id
       AND incident_id = NEW.incident_id;

    RETURN NEW;
END;
$$;

CREATE TRIGGER server_incident_outcome_timeline
AFTER INSERT ON server_operation_outcomes
FOR EACH ROW EXECUTE FUNCTION server_track_incident_outcome();

COMMENT ON COLUMN server_operation_outcomes.incident_id IS
    'Exact incident episode supplied and revalidated before the operation; NULL for generic server actions.';
COMMENT ON TABLE server_incident_events IS
    'Material detector lifecycle and explicitly linked operation outcome edges for a Server incident episode.';
