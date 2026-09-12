CREATE TABLE keel_account (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    parent_id   TEXT REFERENCES keel_account(id),
    path        TEXT NOT NULL,
    state       TEXT NOT NULL DEFAULT 'active',
    external_id TEXT,
    origin      TEXT,
    created_at  TIMESTAMPTZ NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL,
    UNIQUE (origin, external_id)
);

CREATE INDEX keel_account_parent_id ON keel_account (parent_id);
CREATE INDEX keel_account_path ON keel_account (path text_pattern_ops);

CREATE TABLE keel_principal (
    id          TEXT PRIMARY KEY,
    account_id  TEXT NOT NULL REFERENCES keel_account(id),
    login       TEXT NOT NULL UNIQUE,
    level       TEXT NOT NULL,
    role        TEXT NOT NULL DEFAULT 'owner',
    state       TEXT NOT NULL DEFAULT 'active',
    lang        TEXT NOT NULL DEFAULT 'en',
    external_id TEXT,
    origin      TEXT,
    acl_gen     INTEGER NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL,
    UNIQUE (origin, external_id)
);

CREATE INDEX keel_principal_account_id ON keel_principal (account_id);

CREATE TABLE keel_limit (
    account_id TEXT NOT NULL REFERENCES keel_account(id),
    key        TEXT NOT NULL,
    value      JSONB NOT NULL,
    PRIMARY KEY (account_id, key)
);

CREATE TABLE keel_module (
    id           TEXT PRIMARY KEY,
    version      TEXT NOT NULL,
    state        TEXT NOT NULL,
    installed_at TIMESTAMPTZ NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL
);

CREATE TABLE keel_kv (
    key   TEXT PRIMARY KEY,
    value JSONB NOT NULL
);

CREATE TABLE keel_outbox (
    seq          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id           TEXT NOT NULL UNIQUE,
    type         TEXT NOT NULL,
    at           TIMESTAMPTZ NOT NULL,
    actor_json   JSONB,
    subject_json JSONB,
    data_json    JSONB NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL
);

CREATE TABLE keel_idempotency (
    principal_id  TEXT NOT NULL,
    key           TEXT NOT NULL,
    func          TEXT NOT NULL,
    request_hash  TEXT NOT NULL,
    status        TEXT NOT NULL,
    response_json JSONB NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL,
    expires_at    TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (principal_id, key)
);

CREATE INDEX keel_idempotency_expires_at ON keel_idempotency (expires_at);
