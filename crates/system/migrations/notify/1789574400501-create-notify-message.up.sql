CREATE TABLE notify_message (
    id           TEXT PRIMARY KEY,
    principal_id TEXT NOT NULL,
    account_id   TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    key          TEXT NOT NULL,
    params_json  JSONB NOT NULL,
    level        TEXT NOT NULL,
    source       TEXT NOT NULL,
    problem_id   TEXT REFERENCES notify_problem(id) ON DELETE RESTRICT,
    read_at      TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL,
    unread       BOOLEAN NOT NULL GENERATED ALWAYS AS (read_at IS NULL) STORED
);

CREATE INDEX notify_message_principal_id_created_at ON notify_message (principal_id, created_at);
CREATE INDEX notify_message_account_id ON notify_message (account_id);
