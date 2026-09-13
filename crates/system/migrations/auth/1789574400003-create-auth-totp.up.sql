CREATE TABLE auth_totp (
    principal_id TEXT PRIMARY KEY REFERENCES keel_principal(id) ON DELETE RESTRICT,
    secret       TEXT NOT NULL,
    confirmed_at TIMESTAMPTZ,
    last_step    BIGINT,
    created_at   TIMESTAMPTZ NOT NULL
);
