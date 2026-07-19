-- CORE-T3 first consumer: a tenant-pinned post-mutation projection that Core
-- can read after alert_state DELETE triggers close the incident.

CREATE VIEW server_incident_knowledge_snapshots AS
SELECT
    incident.tenant_id,
    incident.incident_id,
    incident.revision,
    concat('Incident ', left(incident.incident_id::TEXT, 8), ': ', incident.message) AS title,
    jsonb_build_object(
        'incident', jsonb_build_object(
            'incident_id', incident.incident_id,
            'revision', incident.revision,
            'fingerprint', incident.fingerprint,
            'host_id', incident.host_id,
            'rule_key', incident.rule_key,
            'severity', incident.severity,
            'message', incident.message,
            'source_state', incident.source_state,
            'status', incident.status,
            'opened_at', incident.opened_at,
            'last_observed_at', incident.last_observed_at,
            'ended_at', incident.ended_at,
            'close_reason', incident.close_reason
        ),
        'timeline', COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'event_id', event.event_id,
                    'event_kind', event.event_kind,
                    'occurred_at', event.occurred_at,
                    'summary', event.summary,
                    'details', event.details
                ) ORDER BY event.occurred_at, event.event_id
            )
            FROM server_incident_events AS event
            WHERE event.tenant_id = incident.tenant_id
              AND event.incident_id = incident.incident_id
        ), '[]'::JSONB),
        'outcome_refs', COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'operation_id', outcome.operation_id,
                    'action', outcome.action,
                    'observed_outcome', outcome.observed_outcome,
                    'experience_revision', outcome.experience_revision,
                    'created_at', outcome.created_at
                ) ORDER BY outcome.created_at, outcome.id
            )
            FROM server_operation_outcomes AS outcome
            WHERE outcome.tenant_id = incident.tenant_id
              AND outcome.incident_id = incident.incident_id
        ), '[]'::JSONB)
    ) AS snapshot
FROM server_incidents AS incident
WHERE incident.status = 'closed';

GRANT SELECT ON server_incident_knowledge_snapshots TO gadgetron_app;

COMMENT ON VIEW server_incident_knowledge_snapshots IS
    'Signed post-mutation incident snapshot projection for the Core Knowledge event outbox.';
