UPDATE log_findings
SET cause = COALESCE(
        NULLIF(BTRIM(cause), ''),
        CASE category
            WHEN 'resource-exhaustion' THEN
                'The kernel could not satisfy a memory allocation and may have terminated a process.'
            WHEN 'storage-failure' THEN
                'A storage device, path or filesystem request failed to complete successfully.'
            WHEN 'service-failure' THEN
                'A service or device reported a failed operation; the exact unit and exit reason are in the log evidence.'
            ELSE
                'A component reported a warning or error that needs correlation with current service and host state.'
        END
    ),
    solution = COALESCE(
        NULLIF(BTRIM(solution), ''),
        CASE category
            WHEN 'resource-exhaustion' THEN
                'Check the affected process, memory pressure, swap and cgroup limits; stabilize the workload before restarting it.'
            WHEN 'storage-failure' THEN
                'Check the affected device and path health, preserve data, and avoid writes or rejoining until storage checks pass.'
            WHEN 'service-failure' THEN
                'Inspect the affected unit or device status and nearby logs, then verify dependencies before retrying or restarting.'
            ELSE
                'Inspect the full bounded evidence and adjacent telemetry, then verify whether the condition persists before acting.'
        END
    )
WHERE classified_by = 'rule'
  AND (
      cause IS NULL OR BTRIM(cause) = ''
      OR solution IS NULL OR BTRIM(solution) = ''
  );
