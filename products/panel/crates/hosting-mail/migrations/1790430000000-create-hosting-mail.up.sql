CREATE TABLE hosting_mail_domain (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    name       TEXT NOT NULL UNIQUE,
    state      TEXT NOT NULL CHECK (state IN ('pending', 'active', 'error')),
    error      TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX hosting_mail_domain_account_id ON hosting_mail_domain (account_id);

CREATE TABLE hosting_mailbox (
    id            TEXT PRIMARY KEY,
    account_id    TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    domain_id     TEXT NOT NULL REFERENCES hosting_mail_domain(id) ON DELETE RESTRICT,
    local         TEXT NOT NULL,
    address       TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    quota         INTEGER NOT NULL CHECK (quota >= 0),
    aliases       JSONB NOT NULL DEFAULT '[]'::jsonb,
    forwards      JSONB NOT NULL DEFAULT '[]'::jsonb,
    suspended     BOOLEAN NOT NULL DEFAULT false,
    state         TEXT NOT NULL CHECK (state IN ('pending', 'active', 'error')),
    error         TEXT,
    created_at    TIMESTAMPTZ NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL,
    UNIQUE (domain_id, local)
);

CREATE INDEX hosting_mailbox_account_id ON hosting_mailbox (account_id);
