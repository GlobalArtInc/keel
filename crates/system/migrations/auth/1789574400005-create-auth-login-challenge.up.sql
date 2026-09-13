CREATE TABLE auth_login_challenge (
    challenge_hash TEXT PRIMARY KEY,
    principal_id   TEXT NOT NULL REFERENCES keel_principal(id) ON DELETE RESTRICT,
    ip             TEXT,
    expires_at     TIMESTAMPTZ NOT NULL,
    used_at        TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL
);

CREATE INDEX auth_login_challenge_principal_id ON auth_login_challenge (principal_id);
