CREATE TABLE beta_thing (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id),
    name       TEXT NOT NULL
);
