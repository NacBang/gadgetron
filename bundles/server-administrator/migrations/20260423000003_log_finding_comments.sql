-- Comments on log findings. Users and Penny both post here — the
-- thread lets operators capture tried-and-failed workarounds, hunches,
-- and "next time try X" notes next to the incident itself.
--
-- author_kind distinguishes human vs agent authorship:
--   'user'  — author_user_id references users.id
--   'penny' — author_user_id is NULL (no user row for the agent);
--             tenant_id + finding_id still scope the comment
--
-- Delete authorization lives in the handler: self-delete iff author,
-- admin can delete anyone's (including Penny's).

CREATE TABLE IF NOT EXISTS log_finding_comments (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL,
    finding_id      UUID NOT NULL REFERENCES log_findings(id) ON DELETE CASCADE,
    author_kind     TEXT NOT NULL CHECK (author_kind IN ('user', 'penny')),
    author_user_id  UUID REFERENCES users(id),
    body            TEXT NOT NULL CHECK (LENGTH(body) BETWEEN 1 AND 4000),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- user comments must carry a user id; Penny comments must not.
    CONSTRAINT log_finding_comments_author_shape
      CHECK ((author_kind = 'user' AND author_user_id IS NOT NULL)
          OR (author_kind = 'penny' AND author_user_id IS NULL))
);

CREATE INDEX IF NOT EXISTS log_finding_comments_finding_created_idx
    ON log_finding_comments (finding_id, created_at DESC);

CREATE INDEX IF NOT EXISTS log_finding_comments_tenant_created_idx
    ON log_finding_comments (tenant_id, created_at DESC);
