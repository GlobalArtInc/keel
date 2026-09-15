CREATE TABLE shelf_book (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id),
    title      TEXT NOT NULL,
    pages      INTEGER NOT NULL DEFAULT 0,
    note       TEXT NOT NULL DEFAULT '',
    UNIQUE (account_id, title)
);
