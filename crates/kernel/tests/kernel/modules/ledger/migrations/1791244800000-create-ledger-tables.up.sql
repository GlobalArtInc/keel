CREATE TABLE ledger_entry (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id),
    label      TEXT NOT NULL,
    amount     BIGINT NOT NULL
);

CREATE TABLE ledger_job (
    id    TEXT PRIMARY KEY,
    label TEXT NOT NULL,
    woken BOOLEAN NOT NULL DEFAULT false
);
