CREATE TABLE dns_template (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES keel_account(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    records    JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    UNIQUE (account_id, name)
);

INSERT INTO dns_template (id, account_id, name, records, created_at, updated_at) VALUES
    ('00000000000000000000000001', '00000000000000000000000000', 'Web host',
     '[{"type": "A", "name": "@", "value": "{ip}", "ttl": 3600},
       {"type": "AAAA", "name": "@", "value": "{ip6}", "ttl": 3600},
       {"type": "CNAME", "name": "www", "value": "{zone}", "ttl": 3600},
       {"type": "MX", "name": "@", "value": "{zone}", "ttl": 3600, "priority": 10}]'::jsonb,
     now(), now()),
    ('00000000000000000000000002', '00000000000000000000000000', 'Empty', '[]'::jsonb, now(), now());
