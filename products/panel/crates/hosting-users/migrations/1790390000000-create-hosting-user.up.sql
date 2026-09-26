CREATE TABLE hosting_user (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    name       TEXT NOT NULL UNIQUE,
    uid        BIGINT,
    home       TEXT,
    shell      BOOLEAN NOT NULL DEFAULT false,
    state      TEXT NOT NULL CHECK (state IN ('pending', 'active', 'suspended', 'error')),
    error      TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX hosting_user_account_id ON hosting_user (account_id);
