CREATE TABLE auth_recovery_code (
    id           TEXT PRIMARY KEY,
    principal_id TEXT NOT NULL REFERENCES keel_principal(id) ON DELETE RESTRICT,
    code_hash    TEXT NOT NULL,
    used_at      TIMESTAMPTZ,
    UNIQUE (principal_id, code_hash)
);
