CREATE TABLE desk_card (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id),
    title      TEXT NOT NULL,
    color      TEXT NOT NULL DEFAULT 'white',
    priority   INTEGER NOT NULL DEFAULT 0,
    pin        TEXT NOT NULL DEFAULT ''
);
