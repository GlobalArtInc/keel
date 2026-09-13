CREATE TABLE auth_login_key (
    key_hash     TEXT PRIMARY KEY,
    principal_id TEXT NOT NULL REFERENCES keel_principal(id) ON DELETE RESTRICT,
    issued_by    TEXT NOT NULL,
    redirect     TEXT NOT NULL,
    expires_at   TIMESTAMPTZ NOT NULL,
    used_at      TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL
);

CREATE INDEX auth_login_key_principal_id ON auth_login_key (principal_id);
