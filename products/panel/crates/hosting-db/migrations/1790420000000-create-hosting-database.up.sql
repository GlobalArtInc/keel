CREATE TABLE hosting_database (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    user_id    TEXT NOT NULL REFERENCES hosting_user(id) ON DELETE RESTRICT,
    name       TEXT NOT NULL UNIQUE,
    state      TEXT NOT NULL CHECK (state IN ('pending', 'active', 'error')),
    error      TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX hosting_database_account_id ON hosting_database (account_id);
CREATE INDEX hosting_database_user_id ON hosting_database (user_id);

CREATE TABLE hosting_database_user (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    user_id    TEXT NOT NULL REFERENCES hosting_user(id) ON DELETE RESTRICT,
    name       TEXT NOT NULL UNIQUE,
    state      TEXT NOT NULL CHECK (state IN ('pending', 'active', 'suspended', 'error')),
    error      TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX hosting_database_user_account_id ON hosting_database_user (account_id);
CREATE INDEX hosting_database_user_user_id ON hosting_database_user (user_id);

CREATE TABLE hosting_database_grant (
    database_user_id TEXT NOT NULL REFERENCES hosting_database_user(id) ON DELETE CASCADE,
    database_id      TEXT NOT NULL REFERENCES hosting_database(id) ON DELETE CASCADE,
    PRIMARY KEY (database_user_id, database_id)
);

CREATE INDEX hosting_database_grant_database_id ON hosting_database_grant (database_id);
