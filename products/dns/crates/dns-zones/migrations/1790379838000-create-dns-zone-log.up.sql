CREATE TABLE dns_zone_log (
    id      BIGSERIAL PRIMARY KEY,
    zone_id TEXT NOT NULL REFERENCES dns_zone(id) ON DELETE CASCADE,
    at      TIMESTAMPTZ NOT NULL,
    level   TEXT NOT NULL CHECK (level IN ('info', 'warn', 'error')),
    text    TEXT NOT NULL
);

CREATE INDEX dns_zone_log_zone_id ON dns_zone_log (zone_id, id);
