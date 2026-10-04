ALTER TABLE peer_product
    ADD COLUMN identity BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE peer_identity (
    peer_id      TEXT NOT NULL,
    subject_kind TEXT NOT NULL,
    subject_id   TEXT NOT NULL,
    seq          BIGINT NOT NULL,
    applied_at   TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (peer_id, subject_kind, subject_id)
);
