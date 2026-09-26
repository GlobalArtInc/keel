CREATE TABLE auth_mail_token (
    token_hash   TEXT PRIMARY KEY,
    purpose      TEXT NOT NULL,
    principal_id TEXT NOT NULL REFERENCES keel_principal(id) ON DELETE RESTRICT,
    value        TEXT,
    expires_at   TIMESTAMPTZ NOT NULL,
    used_at      TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL
);

CREATE INDEX auth_mail_token_principal_id ON auth_mail_token (principal_id);
