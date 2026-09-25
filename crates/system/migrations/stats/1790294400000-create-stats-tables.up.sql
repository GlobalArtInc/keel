CREATE TABLE stats_sample (
    key     TEXT NOT NULL,
    subject TEXT NOT NULL,
    at      TIMESTAMPTZ NOT NULL,
    value   DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (key, subject, at)
);

CREATE INDEX stats_sample_at ON stats_sample (at);

CREATE TABLE stats_agg (
    key        TEXT NOT NULL,
    subject    TEXT NOT NULL,
    bucket     TIMESTAMPTZ NOT NULL,
    resolution TEXT NOT NULL CHECK (resolution IN ('hour', 'day')),
    min        DOUBLE PRECISION NOT NULL,
    max        DOUBLE PRECISION NOT NULL,
    sum        DOUBLE PRECISION NOT NULL,
    count      BIGINT NOT NULL,
    PRIMARY KEY (key, subject, resolution, bucket)
);

CREATE INDEX stats_agg_resolution_bucket ON stats_agg (resolution, bucket);
