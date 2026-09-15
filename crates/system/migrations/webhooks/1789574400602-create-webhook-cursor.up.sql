CREATE TABLE webhook_cursor (
    consumer TEXT PRIMARY KEY,
    seq      BIGINT NOT NULL
);
