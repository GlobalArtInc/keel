CREATE TABLE notify_direct (
    id            TEXT PRIMARY KEY,
    principal_id  TEXT NOT NULL,
    channel_id    TEXT NOT NULL REFERENCES notify_channel(id) ON DELETE CASCADE,
    channel       TEXT NOT NULL,
    template      TEXT NOT NULL,
    address       TEXT NOT NULL,
    params_sealed TEXT,
    attempt       INTEGER NOT NULL DEFAULT 0,
    state         TEXT NOT NULL,
    next_at       TIMESTAMPTZ NOT NULL,
    error         TEXT,
    sent_at       TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL
);

CREATE INDEX notify_direct_pending_next_at ON notify_direct (next_at) WHERE state = 'pending';
CREATE INDEX notify_direct_channel_id ON notify_direct (channel_id);
CREATE INDEX notify_direct_principal_id ON notify_direct (principal_id);
