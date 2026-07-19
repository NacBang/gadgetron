DELETE FROM host_metrics
 WHERE metric = 'temp.celsius'
   AND lower(COALESCE(labels ->> 'source', '')) IN ('vddgfx', 'vddnb', 'vddsoc');

WITH cleaned AS (
    SELECT snapshot.tenant_id,
           snapshot.host_id,
           COALESCE(
               jsonb_agg(reading.value ORDER BY reading.ordinality)
                   FILTER (
                       WHERE lower(COALESCE(reading.value ->> 'label', ''))
                             NOT IN ('vddgfx', 'vddnb', 'vddsoc')
                   ),
               '[]'::jsonb
           ) AS temperatures,
           max((reading.value ->> 'celsius')::DOUBLE PRECISION)
               FILTER (
                   WHERE lower(COALESCE(reading.value ->> 'label', ''))
                         NOT IN ('vddgfx', 'vddnb', 'vddsoc')
               ) AS hottest_temperature_c
      FROM host_stats_latest AS snapshot
      LEFT JOIN LATERAL jsonb_array_elements(
          COALESCE(snapshot.stats -> 'temperatures', '[]'::jsonb)
      ) WITH ORDINALITY AS reading(value, ordinality) ON TRUE
     GROUP BY snapshot.tenant_id, snapshot.host_id
)
UPDATE host_stats_latest AS snapshot
   SET stats = jsonb_set(
       jsonb_set(snapshot.stats, '{temperatures}', cleaned.temperatures, true),
       '{summary,hottest_temperature_c}',
       COALESCE(to_jsonb(cleaned.hottest_temperature_c), 'null'::jsonb),
       true
   )
  FROM cleaned
 WHERE snapshot.tenant_id = cleaned.tenant_id
   AND snapshot.host_id = cleaned.host_id
   AND snapshot.stats -> 'temperatures' IS DISTINCT FROM cleaned.temperatures;

COMMENT ON TABLE host_stats_latest IS
    'Latest bounded telemetry snapshot; sensor features retain their typed unit instead of sharing an untyped input suffix.';
