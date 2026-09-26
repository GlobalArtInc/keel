CREATE TABLE auth_principal_address (
    principal_id TEXT NOT NULL REFERENCES keel_principal(id) ON DELETE RESTRICT,
    kind         TEXT NOT NULL,
    value        TEXT NOT NULL,
    verified_at  TIMESTAMPTZ,
    updated_at   TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (principal_id, kind)
);
