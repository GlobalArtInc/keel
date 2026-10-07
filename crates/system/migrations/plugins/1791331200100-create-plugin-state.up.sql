CREATE TABLE plugin_state (
    id           TEXT PRIMARY KEY,
    enabled      BOOLEAN NOT NULL,
    level        TEXT NOT NULL CHECK (level IN ('service', 'user', 'admin')),
    principal_id TEXT NOT NULL REFERENCES keel_principal(id) ON DELETE RESTRICT,
    changed_at   TIMESTAMPTZ NOT NULL,
    changed_by   TEXT NOT NULL
);
