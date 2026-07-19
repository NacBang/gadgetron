-- Durable Server incident episodes derived from the current alert plane.
-- `alert_state` remains the authority for conditions that are active now;
-- these tables preserve material lifecycle edges without changing detector code.

CREATE TABLE IF NOT EXISTS server_incidents (
    tenant_id       UUID        NOT NULL,
    incident_id     UUID        NOT NULL DEFAULT gen_random_uuid(),
    fingerprint     TEXT        NOT NULL,
    host_id         UUID,
    rule_key        TEXT        NOT NULL,
    severity        TEXT        NOT NULL CHECK (
        severity IN ('critical', 'high', 'medium', 'info')
    ),
    message         TEXT        NOT NULL,
    source_state    TEXT        NOT NULL CHECK (source_state IN ('pending', 'firing')),
    status          TEXT        NOT NULL CHECK (status IN ('active', 'closed')),
    opened_at       TIMESTAMPTZ NOT NULL,
    last_observed_at TIMESTAMPTZ NOT NULL,
    ended_at        TIMESTAMPTZ,
    close_reason    TEXT,
    revision        UUID        NOT NULL DEFAULT gen_random_uuid(),
    PRIMARY KEY (tenant_id, incident_id),
    CHECK (
        (status = 'active' AND ended_at IS NULL AND close_reason IS NULL)
        OR
        (status = 'closed' AND ended_at IS NOT NULL AND close_reason IS NOT NULL)
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS server_incidents_active_fingerprint_idx
    ON server_incidents (tenant_id, fingerprint)
    WHERE status = 'active';

CREATE INDEX IF NOT EXISTS server_incidents_recent_idx
    ON server_incidents (tenant_id, last_observed_at DESC);

CREATE TABLE IF NOT EXISTS server_incident_events (
    tenant_id    UUID        NOT NULL,
    event_id     UUID        NOT NULL DEFAULT gen_random_uuid(),
    incident_id  UUID        NOT NULL,
    event_kind   TEXT        NOT NULL CHECK (
        event_kind IN ('opened', 'state_changed', 'closed')
    ),
    occurred_at  TIMESTAMPTZ NOT NULL,
    summary      TEXT        NOT NULL,
    details      JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(details) = 'object'
    ),
    PRIMARY KEY (tenant_id, event_id),
    FOREIGN KEY (tenant_id, incident_id)
        REFERENCES server_incidents (tenant_id, incident_id)
);

CREATE INDEX IF NOT EXISTS server_incident_events_timeline_idx
    ON server_incident_events (tenant_id, incident_id, occurred_at, event_id);

INSERT INTO server_incidents (
    tenant_id,
    fingerprint,
    host_id,
    rule_key,
    severity,
    message,
    source_state,
    status,
    opened_at,
    last_observed_at
)
SELECT
    alert.tenant_id,
    alert.fingerprint,
    alert.host_id,
    alert.rule_key,
    alert.severity,
    alert.message,
    alert.state,
    'active',
    COALESCE(alert.active_since, alert.pending_since, alert.last_eval_at),
    alert.last_eval_at
FROM alert_state AS alert
WHERE NOT EXISTS (
    SELECT 1
    FROM server_incidents AS incident
    WHERE incident.tenant_id = alert.tenant_id
      AND incident.fingerprint = alert.fingerprint
      AND incident.status = 'active'
);

INSERT INTO server_incident_events (
    tenant_id,
    incident_id,
    event_kind,
    occurred_at,
    summary,
    details
)
SELECT
    incident.tenant_id,
    incident.incident_id,
    'opened',
    incident.opened_at,
    incident.message,
    jsonb_build_object(
        'severity', incident.severity,
        'source_state', incident.source_state,
        'backfilled', true
    )
FROM server_incidents AS incident
WHERE incident.status = 'active'
  AND NOT EXISTS (
      SELECT 1
      FROM server_incident_events AS event
      WHERE event.tenant_id = incident.tenant_id
        AND event.incident_id = incident.incident_id
  );

CREATE OR REPLACE FUNCTION server_track_alert_incident()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    current_incident_id UUID;
    material_change BOOLEAN;
BEGIN
    IF TG_OP = 'DELETE' THEN
        SELECT incident_id
          INTO current_incident_id
          FROM server_incidents
         WHERE tenant_id = OLD.tenant_id
           AND fingerprint = OLD.fingerprint
           AND status = 'active'
         FOR UPDATE;

        IF current_incident_id IS NOT NULL THEN
            UPDATE server_incidents
               SET status = 'closed',
                   ended_at = GREATEST(last_observed_at, now()),
                   close_reason = 'condition_no_longer_active',
                   revision = gen_random_uuid()
             WHERE tenant_id = OLD.tenant_id
               AND incident_id = current_incident_id;

            INSERT INTO server_incident_events (
                tenant_id,
                incident_id,
                event_kind,
                occurred_at,
                summary,
                details
            ) VALUES (
                OLD.tenant_id,
                current_incident_id,
                'closed',
                GREATEST(OLD.last_eval_at, now()),
                'Condition is no longer active',
                jsonb_build_object(
                    'reason', 'condition_no_longer_active',
                    'source_state', OLD.state
                )
            );
        END IF;
        RETURN OLD;
    END IF;

    SELECT incident_id
      INTO current_incident_id
      FROM server_incidents
     WHERE tenant_id = NEW.tenant_id
       AND fingerprint = NEW.fingerprint
       AND status = 'active'
     FOR UPDATE;

    IF current_incident_id IS NULL THEN
        INSERT INTO server_incidents (
            tenant_id,
            fingerprint,
            host_id,
            rule_key,
            severity,
            message,
            source_state,
            status,
            opened_at,
            last_observed_at
        ) VALUES (
            NEW.tenant_id,
            NEW.fingerprint,
            NEW.host_id,
            NEW.rule_key,
            NEW.severity,
            NEW.message,
            NEW.state,
            'active',
            COALESCE(NEW.active_since, NEW.pending_since, NEW.last_eval_at),
            NEW.last_eval_at
        )
        RETURNING incident_id INTO current_incident_id;

        INSERT INTO server_incident_events (
            tenant_id,
            incident_id,
            event_kind,
            occurred_at,
            summary,
            details
        ) VALUES (
            NEW.tenant_id,
            current_incident_id,
            'opened',
            COALESCE(NEW.active_since, NEW.pending_since, NEW.last_eval_at),
            NEW.message,
            jsonb_build_object(
                'severity', NEW.severity,
                'source_state', NEW.state
            )
        );
        RETURN NEW;
    END IF;

    material_change := false;
    IF TG_OP = 'UPDATE' THEN
        material_change :=
            NEW.state IS DISTINCT FROM OLD.state
            OR NEW.severity IS DISTINCT FROM OLD.severity
            OR NEW.message IS DISTINCT FROM OLD.message
            OR NEW.rule_key IS DISTINCT FROM OLD.rule_key
            OR NEW.host_id IS DISTINCT FROM OLD.host_id;
    END IF;

    UPDATE server_incidents
       SET host_id = NEW.host_id,
           rule_key = NEW.rule_key,
           severity = NEW.severity,
           message = NEW.message,
           source_state = NEW.state,
           last_observed_at = GREATEST(last_observed_at, NEW.last_eval_at),
           revision = CASE WHEN material_change THEN gen_random_uuid() ELSE revision END
     WHERE tenant_id = NEW.tenant_id
       AND incident_id = current_incident_id;

    IF material_change THEN
        INSERT INTO server_incident_events (
            tenant_id,
            incident_id,
            event_kind,
            occurred_at,
            summary,
            details
        ) VALUES (
            NEW.tenant_id,
            current_incident_id,
            'state_changed',
            NEW.last_eval_at,
            NEW.message,
            jsonb_build_object(
                'severity', NEW.severity,
                'source_state', NEW.state
            )
        );
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS server_alert_incident_lifecycle ON alert_state;
CREATE TRIGGER server_alert_incident_lifecycle
AFTER INSERT OR UPDATE OR DELETE ON alert_state
FOR EACH ROW EXECUTE FUNCTION server_track_alert_incident();

COMMENT ON TABLE alert_state IS
    'Current pending or firing detector state. Durable lifecycle history is preserved in server_incidents and server_incident_events.';
COMMENT ON TABLE server_incidents IS
    'Durable Server incident episodes. Closed means the source condition is no longer active, not that remediation was verified.';
COMMENT ON TABLE server_incident_events IS
    'Material opened, state-changed and closed edges for a Server incident episode; periodic evaluation is not duplicated.';
