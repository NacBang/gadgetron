-- Correlate only detector-scoped, concurrently active signals into one incident.
-- Existing episodes remain valid and are backfilled as one unscoped signal.

ALTER TABLE alert_state
    ADD COLUMN incident_scope TEXT CHECK (
        incident_scope IS NULL
        OR incident_scope ~ '^[a-z0-9]+(?:[.:_-][a-z0-9]+)*$'
    );

CREATE TABLE server_incident_signals (
    tenant_id        UUID        NOT NULL,
    signal_id        UUID        NOT NULL DEFAULT gen_random_uuid(),
    incident_id      UUID        NOT NULL,
    fingerprint      TEXT        NOT NULL,
    host_id          UUID,
    incident_scope   TEXT,
    rule_key         TEXT        NOT NULL,
    severity         TEXT        NOT NULL CHECK (
        severity IN ('critical', 'high', 'medium', 'info')
    ),
    message          TEXT        NOT NULL,
    source_state     TEXT        NOT NULL CHECK (source_state IN ('pending', 'firing')),
    attached_at      TIMESTAMPTZ NOT NULL,
    last_observed_at TIMESTAMPTZ NOT NULL,
    ended_at         TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, signal_id),
    FOREIGN KEY (tenant_id, incident_id)
        REFERENCES server_incidents (tenant_id, incident_id),
    CHECK (ended_at IS NULL OR ended_at >= attached_at)
);

CREATE UNIQUE INDEX server_incident_signals_active_fingerprint_idx
    ON server_incident_signals (tenant_id, fingerprint)
    WHERE ended_at IS NULL;

CREATE INDEX server_incident_signals_incident_idx
    ON server_incident_signals (
        tenant_id,
        incident_id,
        ended_at,
        last_observed_at DESC
    );

CREATE INDEX server_incident_signals_scope_idx
    ON server_incident_signals (
        tenant_id,
        host_id,
        incident_scope,
        last_observed_at DESC
    )
    WHERE ended_at IS NULL AND incident_scope IS NOT NULL;

INSERT INTO server_incident_signals (
    tenant_id,
    incident_id,
    fingerprint,
    host_id,
    incident_scope,
    rule_key,
    severity,
    message,
    source_state,
    attached_at,
    last_observed_at,
    ended_at
)
SELECT
    tenant_id,
    incident_id,
    fingerprint,
    host_id,
    NULL,
    rule_key,
    severity,
    message,
    source_state,
    opened_at,
    last_observed_at,
    ended_at
FROM server_incidents;

ALTER TABLE server_incident_events
    DROP CONSTRAINT server_incident_events_event_kind_check;

ALTER TABLE server_incident_events
    ADD CONSTRAINT server_incident_events_event_kind_check CHECK (
        event_kind IN (
            'opened',
            'state_changed',
            'signal_attached',
            'signal_changed',
            'signal_cleared',
            'closed',
            'action_succeeded',
            'action_failed',
            'action_indeterminate',
            'experience_recorded'
        )
    );

CREATE OR REPLACE FUNCTION server_refresh_incident_from_signals(
    refresh_tenant_id UUID,
    refresh_incident_id UUID,
    observed_at TIMESTAMPTZ,
    change_revision BOOLEAN
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    primary_signal server_incident_signals%ROWTYPE;
    latest_observed_at TIMESTAMPTZ;
BEGIN
    SELECT signal.*
      INTO primary_signal
      FROM server_incident_signals AS signal
     WHERE signal.tenant_id = refresh_tenant_id
       AND signal.incident_id = refresh_incident_id
       AND signal.ended_at IS NULL
     ORDER BY
        CASE signal.severity
            WHEN 'critical' THEN 0
            WHEN 'high' THEN 1
            WHEN 'medium' THEN 2
            ELSE 3
        END,
        CASE signal.source_state WHEN 'firing' THEN 0 ELSE 1 END,
        signal.last_observed_at DESC,
        signal.signal_id
     LIMIT 1;

    IF primary_signal.signal_id IS NULL THEN
        RETURN;
    END IF;

    SELECT max(signal.last_observed_at)
      INTO latest_observed_at
      FROM server_incident_signals AS signal
     WHERE signal.tenant_id = refresh_tenant_id
       AND signal.incident_id = refresh_incident_id
       AND signal.ended_at IS NULL;

    UPDATE server_incidents
       SET fingerprint = primary_signal.fingerprint,
           host_id = primary_signal.host_id,
           rule_key = primary_signal.rule_key,
           severity = primary_signal.severity,
           message = primary_signal.message,
           source_state = primary_signal.source_state,
           last_observed_at = GREATEST(
               server_incidents.last_observed_at,
               latest_observed_at,
               observed_at
           ),
           revision = CASE
               WHEN change_revision THEN gen_random_uuid()
               ELSE server_incidents.revision
           END
     WHERE tenant_id = refresh_tenant_id
       AND incident_id = refresh_incident_id;
END;
$$;

CREATE OR REPLACE FUNCTION server_track_alert_incident()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    current_incident_id UUID;
    candidate_incident_id UUID;
    candidate_count BIGINT;
    remaining_signals BIGINT;
    material_change BOOLEAN;
    event_time TIMESTAMPTZ;
BEGIN
    IF TG_OP = 'DELETE' THEN
        SELECT signal.incident_id
          INTO current_incident_id
          FROM server_incident_signals AS signal
         WHERE signal.tenant_id = OLD.tenant_id
           AND signal.fingerprint = OLD.fingerprint
           AND signal.ended_at IS NULL
         FOR UPDATE;

        IF current_incident_id IS NULL THEN
            RETURN OLD;
        END IF;

        event_time := GREATEST(OLD.last_eval_at, now());
        UPDATE server_incident_signals
           SET last_observed_at = GREATEST(last_observed_at, OLD.last_eval_at),
               ended_at = event_time
         WHERE tenant_id = OLD.tenant_id
           AND fingerprint = OLD.fingerprint
           AND ended_at IS NULL;

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
            'signal_cleared',
            event_time,
            OLD.message,
            jsonb_build_object(
                'fingerprint', OLD.fingerprint,
                'rule_key', OLD.rule_key,
                'incident_scope', OLD.incident_scope,
                'source_state', OLD.state
            )
        );

        SELECT count(*)
          INTO remaining_signals
          FROM server_incident_signals AS signal
         WHERE signal.tenant_id = OLD.tenant_id
           AND signal.incident_id = current_incident_id
           AND signal.ended_at IS NULL;

        IF remaining_signals = 0 THEN
            UPDATE server_incidents
               SET status = 'closed',
                   ended_at = event_time,
                   last_observed_at = GREATEST(last_observed_at, OLD.last_eval_at),
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
                event_time,
                'All correlated signals are no longer active',
                jsonb_build_object(
                    'reason', 'condition_no_longer_active',
                    'remaining_signals', 0
                )
            );
        ELSE
            PERFORM server_refresh_incident_from_signals(
                OLD.tenant_id,
                current_incident_id,
                event_time,
                true
            );
        END IF;
        RETURN OLD;
    END IF;

    IF TG_OP = 'UPDATE' THEN
        SELECT signal.incident_id
          INTO current_incident_id
          FROM server_incident_signals AS signal
         WHERE signal.tenant_id = NEW.tenant_id
           AND signal.fingerprint = NEW.fingerprint
           AND signal.ended_at IS NULL
         FOR UPDATE;

        IF current_incident_id IS NULL THEN
            RAISE EXCEPTION 'active incident signal is missing for alert %', NEW.fingerprint;
        END IF;

        material_change :=
            NEW.state IS DISTINCT FROM OLD.state
            OR NEW.severity IS DISTINCT FROM OLD.severity
            OR NEW.message IS DISTINCT FROM OLD.message
            OR NEW.rule_key IS DISTINCT FROM OLD.rule_key
            OR NEW.host_id IS DISTINCT FROM OLD.host_id
            OR NEW.incident_scope IS DISTINCT FROM OLD.incident_scope;

        UPDATE server_incident_signals
           SET host_id = NEW.host_id,
               incident_scope = NEW.incident_scope,
               rule_key = NEW.rule_key,
               severity = NEW.severity,
               message = NEW.message,
               source_state = NEW.state,
               last_observed_at = GREATEST(last_observed_at, NEW.last_eval_at)
         WHERE tenant_id = NEW.tenant_id
           AND fingerprint = NEW.fingerprint
           AND ended_at IS NULL;

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
                'signal_changed',
                NEW.last_eval_at,
                NEW.message,
                jsonb_build_object(
                    'fingerprint', NEW.fingerprint,
                    'rule_key', NEW.rule_key,
                    'incident_scope', NEW.incident_scope,
                    'severity', NEW.severity,
                    'source_state', NEW.state
                )
            );
        END IF;

        PERFORM server_refresh_incident_from_signals(
            NEW.tenant_id,
            current_incident_id,
            NEW.last_eval_at,
            material_change
        );
        RETURN NEW;
    END IF;

    IF NEW.incident_scope IS NOT NULL AND NEW.host_id IS NOT NULL THEN
        PERFORM pg_advisory_xact_lock(
            hashtextextended(
                NEW.tenant_id::text || ':' || NEW.host_id::text || ':' || NEW.incident_scope,
                0
            )
        );
        SELECT count(*), max(candidate.incident_id::text)::uuid
          INTO candidate_count, candidate_incident_id
          FROM (
              SELECT DISTINCT signal.incident_id
                FROM server_incident_signals AS signal
                JOIN server_incidents AS incident
                  ON incident.tenant_id = signal.tenant_id
                 AND incident.incident_id = signal.incident_id
               WHERE signal.tenant_id = NEW.tenant_id
                 AND signal.host_id = NEW.host_id
                 AND signal.incident_scope = NEW.incident_scope
                 AND signal.ended_at IS NULL
                 AND incident.status = 'active'
               LIMIT 2
          ) AS candidate;
        IF candidate_count = 1 THEN
            current_incident_id := candidate_incident_id;
        END IF;
    END IF;

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
                'fingerprint', NEW.fingerprint,
                'rule_key', NEW.rule_key,
                'incident_scope', NEW.incident_scope,
                'severity', NEW.severity,
                'source_state', NEW.state
            )
        );
    ELSE
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
            'signal_attached',
            COALESCE(NEW.active_since, NEW.pending_since, NEW.last_eval_at),
            NEW.message,
            jsonb_build_object(
                'fingerprint', NEW.fingerprint,
                'rule_key', NEW.rule_key,
                'incident_scope', NEW.incident_scope,
                'severity', NEW.severity,
                'source_state', NEW.state
            )
        );
    END IF;

    INSERT INTO server_incident_signals (
        tenant_id,
        incident_id,
        fingerprint,
        host_id,
        incident_scope,
        rule_key,
        severity,
        message,
        source_state,
        attached_at,
        last_observed_at
    ) VALUES (
        NEW.tenant_id,
        current_incident_id,
        NEW.fingerprint,
        NEW.host_id,
        NEW.incident_scope,
        NEW.rule_key,
        NEW.severity,
        NEW.message,
        NEW.state,
        COALESCE(NEW.active_since, NEW.pending_since, NEW.last_eval_at),
        NEW.last_eval_at
    );

    PERFORM server_refresh_incident_from_signals(
        NEW.tenant_id,
        current_incident_id,
        NEW.last_eval_at,
        true
    );
    RETURN NEW;
END;
$$;

COMMENT ON COLUMN alert_state.incident_scope IS
    'Detector-owned bounded correlation scope. NULL signals are never automatically merged.';
COMMENT ON TABLE server_incident_signals IS
    'Exact alert signals attached to one incident while their bounded server scope overlaps.';
COMMENT ON TABLE server_incident_events IS
    'Material signal lifecycle, exact operation outcomes and learning handoffs for a Server incident episode.';
