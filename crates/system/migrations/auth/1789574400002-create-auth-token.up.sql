CREATE TABLE auth_token (
    id           TEXT PRIMARY KEY,
    token_hash   TEXT NOT NULL UNIQUE,
    principal_id TEXT NOT NULL REFERENCES keel_principal(id) ON DELETE RESTRICT,
    account_id   TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    name         TEXT NOT NULL,
    level        TEXT NOT NULL,
    scope        JSONB NOT NULL,
    ip_allow     JSONB NOT NULL,
    expires_at   TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL,
    revoked_at   TIMESTAMPTZ
);

CREATE INDEX auth_token_principal_id ON auth_token (principal_id);
CREATE INDEX auth_token_account_id ON auth_token (account_id);
