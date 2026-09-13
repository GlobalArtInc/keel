CREATE TABLE auth_session (
    id_hash            TEXT PRIMARY KEY,
    principal_id       TEXT NOT NULL REFERENCES keel_principal(id) ON DELETE RESTRICT,
    account_id         TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    actor_principal_id TEXT REFERENCES keel_principal(id) ON DELETE RESTRICT,
    kind               TEXT NOT NULL,
    ip                 TEXT,
    user_agent         TEXT,
    created_at         TIMESTAMPTZ NOT NULL,
    idle_until         TIMESTAMPTZ NOT NULL,
    absolute_until     TIMESTAMPTZ NOT NULL,
    revoked_at         TIMESTAMPTZ,
    reason             TEXT
);

CREATE INDEX auth_session_principal_id ON auth_session (principal_id);
