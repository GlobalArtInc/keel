CREATE TABLE auth_grant (
    id           TEXT PRIMARY KEY,
    principal_id TEXT NOT NULL REFERENCES keel_principal(id) ON DELETE RESTRICT,
    account_id   TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    key          TEXT NOT NULL,
    granted      BOOLEAN NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL,
    created_by   TEXT NOT NULL,
    UNIQUE (principal_id, key)
);

CREATE INDEX auth_grant_account_id ON auth_grant (account_id);
