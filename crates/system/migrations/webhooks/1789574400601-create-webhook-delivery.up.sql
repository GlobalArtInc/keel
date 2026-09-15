CREATE TABLE webhook_delivery (
    id              TEXT PRIMARY KEY,
    subscription_id TEXT NOT NULL REFERENCES webhook_subscription(id) ON DELETE CASCADE,
    account_id      TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    event_id        TEXT NOT NULL,
    seq             BIGINT NOT NULL,
    type            TEXT NOT NULL,
    attempt         INTEGER NOT NULL DEFAULT 0,
    state           TEXT NOT NULL,
    next_at         TIMESTAMPTZ NOT NULL,
    last_status     INTEGER,
    last_error      TEXT,
    delivered_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL
);

CREATE UNIQUE INDEX webhook_delivery_subscription_event ON webhook_delivery (subscription_id, event_id);
CREATE INDEX webhook_delivery_due ON webhook_delivery (next_at) WHERE state IN ('pending', 'failed');
CREATE INDEX webhook_delivery_account_id ON webhook_delivery (account_id);
