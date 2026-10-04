ALTER TABLE dns_zone ADD COLUMN suspended BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE dns_account (
    account_id TEXT PRIMARY KEY REFERENCES keel_account(id) ON DELETE RESTRICT,
    template   TEXT,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE dns_service (
    service_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    seq        BIGINT GENERATED ALWAYS AS IDENTITY,
    zone_limit INTEGER CHECK (zone_limit >= 0),
    template   TEXT,
    held       BOOLEAN NOT NULL,
    closed     BOOLEAN NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX dns_service_account_id ON dns_service (account_id);
