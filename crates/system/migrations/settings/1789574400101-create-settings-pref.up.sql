CREATE TABLE settings_pref (
    principal_id TEXT NOT NULL REFERENCES keel_principal(id) ON DELETE RESTRICT,
    key          TEXT NOT NULL,
    value        JSONB NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (principal_id, key)
);
