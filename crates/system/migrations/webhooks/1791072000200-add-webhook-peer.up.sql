ALTER TABLE webhook_subscription
    ADD COLUMN peer_id TEXT,
    ADD COLUMN level TEXT,
    ADD COLUMN floor_seq BIGINT,
    ADD COLUMN local_only BOOLEAN NOT NULL DEFAULT FALSE;

CREATE UNIQUE INDEX webhook_subscription_peer_id ON webhook_subscription (peer_id) WHERE peer_id IS NOT NULL;
