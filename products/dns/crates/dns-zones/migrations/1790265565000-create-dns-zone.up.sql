CREATE TABLE dns_zone (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    name       TEXT NOT NULL UNIQUE,
    kind       TEXT NOT NULL DEFAULT 'primary' CHECK (kind = 'primary'),
    serial     BIGINT NOT NULL,
    refresh    INTEGER NOT NULL,
    retry      INTEGER NOT NULL,
    expire     INTEGER NOT NULL,
    minimum    INTEGER NOT NULL,
    template   TEXT,
    state      TEXT NOT NULL CHECK (state IN ('pending', 'active', 'error')),
    error      TEXT,
    served     BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX dns_zone_account_id ON dns_zone (account_id);
