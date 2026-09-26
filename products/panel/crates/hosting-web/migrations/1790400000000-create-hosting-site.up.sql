CREATE TABLE hosting_site (
    id          TEXT PRIMARY KEY,
    account_id  TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    user_id     TEXT NOT NULL REFERENCES hosting_user(id) ON DELETE RESTRICT,
    domain      TEXT NOT NULL UNIQUE,
    aliases     JSONB NOT NULL DEFAULT '[]'::jsonb,
    root        TEXT NOT NULL,
    php         TEXT,
    php_applied TEXT,
    suspended   BOOLEAN NOT NULL DEFAULT false,
    state       TEXT NOT NULL CHECK (state IN ('pending', 'active', 'error')),
    error       TEXT,
    created_at  TIMESTAMPTZ NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL
);

CREATE INDEX hosting_site_account_id ON hosting_site (account_id);
CREATE INDEX hosting_site_user_id ON hosting_site (user_id);
