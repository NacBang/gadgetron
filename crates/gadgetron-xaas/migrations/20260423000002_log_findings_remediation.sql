-- log_findings: cause + solution + optional executable remediation.
--
-- `cause` and `solution` are free-form text that Penny / the rule
-- engine fills in for the operator. `remediation` is a structured
-- "click-to-run" descriptor: when non-null AND the embedded `tool`
-- string is in the frontend's whitelist (server.systemctl /
-- server.apt only, no arbitrary shell), the UI renders a "✓ 승인 실행"
-- button that POSTs to the matching gadget action.

ALTER TABLE log_findings
    ADD COLUMN IF NOT EXISTS cause TEXT,
    ADD COLUMN IF NOT EXISTS solution TEXT,
    ADD COLUMN IF NOT EXISTS remediation JSONB;
