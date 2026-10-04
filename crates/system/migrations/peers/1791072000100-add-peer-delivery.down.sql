DROP TABLE peer_inbox;

ALTER TABLE peer_product
    DROP COLUMN feed_seq,
    DROP COLUMN pull_seq,
    DROP COLUMN events;
