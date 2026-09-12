CREATE TABLE note_item (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    title      TEXT NOT NULL,
    body       TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX note_item_account_id ON note_item (account_id);
