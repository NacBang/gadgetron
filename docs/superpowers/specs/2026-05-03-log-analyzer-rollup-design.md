# Log Analyzer Roll-Up Design

## Goal

Make the Logs surface catch recurring SMART disk failures while preventing duplicate findings and screen-filling repeated messages.

## Decisions

- Background log scanning is on by default in DB mode because the Logs page is an operational monitoring surface. Operators can opt out with `GADGETRON_LOG_ANALYZER_AUTO=0`.
- LLM fallback stays opt-in. Default automatic scanning remains rule-only and does not spend tokens.
- Each classification carries a stable `fingerprint`. Open findings are unique by `(tenant_id, host_id, source, fingerprint)`.
- Repeated sightings of the same open fingerprint update one row by incrementing `count`, updating `ts_last`, and replacing `excerpt` with the latest line.
- Dismissed findings keep the existing mute behavior for seven days, keyed by fingerprint, so rescanning historical log lines does not immediately recreate a card.
- Initial journal scan looks back six hours instead of thirty minutes. Follow-up scans use journal cursor tokens.

## SMART Coverage

The rule set must classify:

- `smartd ... Currently unreadable (pending) sectors` as a critical disk-health finding.
- `smartd ... Offline uncorrectable sectors` as the same critical disk-health finding for the same device.
- `smartd ... /usr/bin/mail ... mailx or mailutils package` and `run-parts ... 10mail exited with return code 1` as a separate smartd alert-delivery finding.

Device-specific SMART disk-health findings use `smartd_disk_health:<device>` as the fingerprint, for example `smartd_disk_health:/dev/sdb`.

## UI Behavior

The Logs empty state must distinguish "never scanned" from "scanned and no open findings." Finding cards should show count and first/last seen metadata rather than repeated cards.

## Tests

- Rule tests cover SMART pending sectors, offline uncorrectable sectors, and alert-delivery failure.
- Store behavior is verified against local Postgres after migration by running `loganalysis.scan_now` and checking that repeated SMART lines roll up into a single finding row.
- Web tests should keep rendering the Logs page with the new optional `fingerprint` field.
