CREATE TABLE svc_service (
    id                TEXT PRIMARY KEY,
    kind              TEXT NOT NULL,
    external_id       TEXT NOT NULL,
    account_id        TEXT NOT NULL REFERENCES keel_account(id) ON DELETE RESTRICT,
    status            TEXT NOT NULL CHECK (status IN ('active', 'suspended', 'closed')),
    params_json       JSONB NOT NULL,
    state_json        JSONB NOT NULL,
    schema_version    INTEGER NOT NULL,
    peer_id           TEXT,
    retain_until      TIMESTAMPTZ,
    suspended_account BOOLEAN NOT NULL DEFAULT false,
    created_at        TIMESTAMPTZ NOT NULL,
    updated_at        TIMESTAMPTZ NOT NULL,
    UNIQUE (kind, external_id)
);

CREATE INDEX svc_service_account_id ON svc_service (account_id);
