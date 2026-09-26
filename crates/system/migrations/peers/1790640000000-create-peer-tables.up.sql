CREATE TABLE peer_product (
    id                  TEXT PRIMARY KEY,
    instance_id         TEXT NOT NULL UNIQUE,
    products_json       JSONB NOT NULL,
    keel_version        TEXT NOT NULL,
    protocol            INTEGER NOT NULL,
    url                 TEXT,
    public_key          TEXT NOT NULL,
    inbound_token_hash  TEXT NOT NULL UNIQUE,
    inbound_scopes_json JSONB NOT NULL,
    outbound_token_enc  TEXT NOT NULL,
    webhook_secret_enc  TEXT,
    status              TEXT NOT NULL,
    last_seen_at        TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL
);

CREATE TABLE peer_pair_code (
    id          TEXT PRIMARY KEY,
    code_hash   TEXT NOT NULL UNIQUE,
    scopes_json JSONB NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL,
    created_by  TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL
);

CREATE INDEX peer_pair_code_expires_at ON peer_pair_code (expires_at);
