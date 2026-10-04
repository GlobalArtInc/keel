DROP INDEX webhook_subscription_peer_id;

ALTER TABLE webhook_subscription
    DROP COLUMN local_only,
    DROP COLUMN floor_seq,
    DROP COLUMN level,
    DROP COLUMN peer_id;
