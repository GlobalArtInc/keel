CREATE TABLE hosting_certificate (
    id          TEXT PRIMARY KEY,
    account_id  TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    site_id     TEXT NOT NULL UNIQUE REFERENCES hosting_site(id) ON DELETE RESTRICT,
    domain      TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('acme', 'uploaded')),
    state       TEXT NOT NULL CHECK (state IN ('pending', 'active', 'error')),
    error       TEXT,
    installed   TEXT,
    not_after   TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL
);

CREATE INDEX hosting_certificate_account_id ON hosting_certificate (account_id);
CREATE INDEX hosting_certificate_not_after ON hosting_certificate (not_after);

CREATE TABLE hosting_certificate_account (
    directory   TEXT PRIMARY KEY,
    credentials TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL
);
