CREATE TABLE rack_item (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id),
    title      TEXT NOT NULL,
    done       BOOLEAN NOT NULL DEFAULT FALSE
);
