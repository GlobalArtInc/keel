CREATE TABLE settings_value (
    scope_id   TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    key        TEXT NOT NULL,
    value      JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    updated_by TEXT NOT NULL,
    PRIMARY KEY (scope_id, key)
);
