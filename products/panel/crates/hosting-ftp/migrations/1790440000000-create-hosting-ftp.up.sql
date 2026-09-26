CREATE TABLE hosting_ftp_user (
    id            TEXT PRIMARY KEY,
    account_id    TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    site_id       TEXT NOT NULL REFERENCES hosting_site(id) ON DELETE RESTRICT,
    domain        TEXT NOT NULL,
    login         TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    directory     TEXT NOT NULL DEFAULT '',
    suspended     BOOLEAN NOT NULL DEFAULT false,
    state         TEXT NOT NULL CHECK (state IN ('pending', 'active', 'error')),
    error         TEXT,
    created_at    TIMESTAMPTZ NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL
);

CREATE INDEX hosting_ftp_user_account_id ON hosting_ftp_user (account_id);
CREATE INDEX hosting_ftp_user_site_id ON hosting_ftp_user (site_id);
