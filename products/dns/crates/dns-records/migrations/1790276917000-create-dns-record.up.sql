CREATE TABLE dns_record (
    id         TEXT PRIMARY KEY,
    zone_id    TEXT NOT NULL REFERENCES dns_zone(id) ON DELETE CASCADE,
    account_id TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    type       TEXT NOT NULL CHECK (type IN ('A', 'AAAA', 'CNAME', 'MX', 'TXT', 'NS', 'SRV', 'CAA')),
    name       TEXT NOT NULL,
    value      TEXT NOT NULL,
    ttl        INTEGER NOT NULL,
    priority   INTEGER,
    weight     INTEGER,
    port       INTEGER,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX dns_record_zone_id ON dns_record (zone_id);
CREATE INDEX dns_record_account_id ON dns_record (account_id);
