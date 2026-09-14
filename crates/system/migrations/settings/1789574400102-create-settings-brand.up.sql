CREATE TABLE settings_brand (
    host       TEXT PRIMARY KEY,
    name       TEXT,
    logo       TEXT,
    hue        INTEGER,
    chroma     DOUBLE PRECISION,
    updated_at TIMESTAMPTZ NOT NULL,
    updated_by TEXT NOT NULL
);
