CREATE TABLE audit_call (
    id                 TEXT PRIMARY KEY,
    at                 TIMESTAMPTZ NOT NULL,
    request_id         TEXT NOT NULL,
    function           TEXT NOT NULL,
    module             TEXT NOT NULL,
    principal_id       TEXT,
    principal_login    TEXT NOT NULL,
    account_id         TEXT NOT NULL,
    actor_principal_id TEXT,
    actor_login        TEXT,
    via                TEXT NOT NULL,
    ip                 TEXT,
    params_redacted    JSONB NOT NULL,
    subject            TEXT,
    after              JSONB,
    status             TEXT NOT NULL,
    error_code         TEXT,
    error_key          TEXT,
    elapsed_ms         BIGINT NOT NULL
);

CREATE INDEX audit_call_at ON audit_call (at);
CREATE INDEX audit_call_account_id_at ON audit_call (account_id, at);
CREATE INDEX audit_call_request_id ON audit_call (request_id);
