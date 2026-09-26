CREATE TABLE hosting_backup (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    site_id    TEXT NOT NULL REFERENCES hosting_site(id) ON DELETE RESTRICT,
    domain     TEXT NOT NULL,
    file       TEXT NOT NULL UNIQUE,
    bytes      BIGINT,
    databases  JSONB NOT NULL DEFAULT '[]',
    mailboxes  JSONB NOT NULL DEFAULT '[]',
    scheduled  BOOLEAN NOT NULL DEFAULT false,
    state      TEXT NOT NULL CHECK (state IN ('pending', 'active', 'error')),
    error      TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX hosting_backup_account_id ON hosting_backup (account_id);
CREATE INDEX hosting_backup_site_id ON hosting_backup (site_id, created_at DESC);

CREATE TABLE hosting_backup_schedule (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    site_id    TEXT NOT NULL UNIQUE REFERENCES hosting_site(id) ON DELETE RESTRICT,
    domain     TEXT NOT NULL,
    schedule   TEXT NOT NULL,
    keep       INTEGER NOT NULL CHECK (keep BETWEEN 1 AND 30),
    next_at    TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX hosting_backup_schedule_account_id ON hosting_backup_schedule (account_id);
CREATE INDEX hosting_backup_schedule_next_at ON hosting_backup_schedule (next_at);
