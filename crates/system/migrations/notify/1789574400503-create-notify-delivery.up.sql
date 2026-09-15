CREATE TABLE notify_delivery (
    id           TEXT PRIMARY KEY,
    channel_id   TEXT NOT NULL REFERENCES notify_channel(id) ON DELETE CASCADE,
    channel      TEXT NOT NULL,
    message_id   TEXT NOT NULL REFERENCES notify_message(id) ON DELETE CASCADE,
    message_key  TEXT NOT NULL,
    attempt      INTEGER NOT NULL DEFAULT 0,
    state        TEXT NOT NULL,
    next_at      TIMESTAMPTZ NOT NULL,
    error        TEXT,
    sent_at      TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL
);

CREATE UNIQUE INDEX notify_delivery_channel_message ON notify_delivery (channel_id, message_id);
CREATE INDEX notify_delivery_pending_next_at ON notify_delivery (next_at) WHERE state = 'pending';
CREATE INDEX notify_delivery_channel_id ON notify_delivery (channel_id);
CREATE INDEX notify_delivery_message_id ON notify_delivery (message_id);
