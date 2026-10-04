CREATE TABLE hosting_site_service (
    service_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    seq        BIGINT GENERATED ALWAYS AS IDENTITY,
    site_limit INTEGER CHECK (site_limit >= 0),
    held       BOOLEAN NOT NULL,
    closed     BOOLEAN NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX hosting_site_service_account_id ON hosting_site_service (account_id);
