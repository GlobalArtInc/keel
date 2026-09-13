CREATE TABLE auth_password (
    principal_id TEXT PRIMARY KEY REFERENCES keel_principal(id) ON DELETE RESTRICT,
    hash         TEXT NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL
);
