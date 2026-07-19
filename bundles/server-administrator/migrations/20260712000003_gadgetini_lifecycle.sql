-- Optimistic lifecycle identity for parent-bound Gadgetini relationships.

ALTER TABLE server_gadgetini_latest
    ADD COLUMN IF NOT EXISTS relation_revision UUID NOT NULL DEFAULT gen_random_uuid();

CREATE UNIQUE INDEX IF NOT EXISTS server_gadgetini_latest_parent_child_idx
    ON server_gadgetini_latest (tenant_id, parent_target_id, gadgetini_id);
