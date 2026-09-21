CREATE TABLE update_run (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    from_version TEXT NOT NULL,
    to_version   TEXT NOT NULL,
    channel      TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    request_id   TEXT NOT NULL,
    dump_dir     TEXT NOT NULL,
    tables       INTEGER NOT NULL,
    requested_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX update_run_to_version_requested_at ON update_run (to_version, requested_at);
