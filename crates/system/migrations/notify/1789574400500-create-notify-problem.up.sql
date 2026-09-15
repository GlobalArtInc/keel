CREATE TABLE notify_problem (
    id          TEXT PRIMARY KEY,
    key         TEXT NOT NULL,
    subject     TEXT NOT NULL,
    severity    TEXT NOT NULL,
    params_json JSONB NOT NULL,
    account_id  TEXT REFERENCES keel_account(id) ON DELETE RESTRICT,
    source      TEXT NOT NULL,
    count       BIGINT NOT NULL DEFAULT 1,
    raised_at   TIMESTAMPTZ NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL,
    cleared_at  TIMESTAMPTZ,
    open        BOOLEAN NOT NULL GENERATED ALWAYS AS (cleared_at IS NULL) STORED
);

CREATE UNIQUE INDEX notify_problem_open_pair ON notify_problem (key, subject) WHERE cleared_at IS NULL;
CREATE INDEX notify_problem_account_id ON notify_problem (account_id);
CREATE INDEX notify_problem_source_open ON notify_problem (source) WHERE cleared_at IS NULL;
