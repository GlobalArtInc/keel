CREATE TABLE auth_ip_rule (
    id         TEXT PRIMARY KEY,
    action     TEXT NOT NULL,
    cidr       TEXT NOT NULL,
    level      TEXT NOT NULL,
    comment    TEXT,
    created_at TIMESTAMPTZ NOT NULL
);
