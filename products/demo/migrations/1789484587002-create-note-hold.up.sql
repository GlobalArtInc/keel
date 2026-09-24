CREATE TABLE note_hold (
    account_id TEXT PRIMARY KEY REFERENCES keel_account(id) ON DELETE RESTRICT,
    held_at    TIMESTAMPTZ NOT NULL
);
