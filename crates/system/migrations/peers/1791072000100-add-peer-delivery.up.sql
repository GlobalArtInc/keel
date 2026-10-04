ALTER TABLE peer_product
    ADD COLUMN events TEXT[] NOT NULL DEFAULT '{}',
    ADD COLUMN pull_seq BIGINT,
    ADD COLUMN feed_seq BIGINT NOT NULL DEFAULT 0;

CREATE TABLE peer_inbox (
    peer_id      TEXT NOT NULL,
    event_id     TEXT NOT NULL,
    event_json   JSONB NOT NULL,
    received_at  TIMESTAMPTZ NOT NULL,
    processed_at TIMESTAMPTZ,
    attempts     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (peer_id, event_id)
);

CREATE INDEX peer_inbox_pending ON peer_inbox (received_at, peer_id, event_id) WHERE processed_at IS NULL;
CREATE INDEX peer_inbox_processed_at ON peer_inbox (processed_at) WHERE processed_at IS NOT NULL;
