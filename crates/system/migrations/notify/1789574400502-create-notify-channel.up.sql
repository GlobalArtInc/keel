CREATE TABLE notify_channel (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    name         TEXT NOT NULL,
    enabled      BOOLEAN NOT NULL,
    config_json  JSONB NOT NULL,
    last_sent_at TIMESTAMPTZ,
    last_error   TEXT,
    created_at   TIMESTAMPTZ NOT NULL,
    created_by   TEXT NOT NULL
);
