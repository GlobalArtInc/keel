CREATE TABLE depot_box (
    service_id TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    account_id TEXT NOT NULL,
    quota      INTEGER NOT NULL,
    active     BOOLEAN NOT NULL,
    note       TEXT
);
