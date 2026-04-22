-- ISSUE 32 — cache the user's profile picture URL (Google OIDC
-- `picture` claim). NULL for password-only users or when the identity
-- provider doesn't expose one.

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS avatar_url TEXT;
