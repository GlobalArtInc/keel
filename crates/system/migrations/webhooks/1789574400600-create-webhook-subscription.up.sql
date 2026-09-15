CREATE TABLE webhook_subscription (
    id           TEXT PRIMARY KEY,
    account_id   TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    principal_id TEXT NOT NULL,
    name         TEXT NOT NULL,
    url          TEXT NOT NULL,
    secret       TEXT NOT NULL,
    events       TEXT[] NOT NULL,
    enabled      BOOLEAN NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL,
    created_by   TEXT NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL
);

CREATE INDEX webhook_subscription_account_id ON webhook_subscription (account_id);
CREATE INDEX webhook_subscription_enabled ON webhook_subscription (enabled) WHERE enabled;
