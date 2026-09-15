CREATE TABLE cron_run (
    id           TEXT PRIMARY KEY,
    job          TEXT NOT NULL,
    scheduled_at TIMESTAMPTZ NOT NULL,
    started_at   TIMESTAMPTZ NOT NULL,
    finished_at  TIMESTAMPTZ,
    outcome      TEXT NOT NULL,
    error        TEXT
);

CREATE INDEX cron_run_job_started_at ON cron_run (job, started_at);
CREATE INDEX cron_run_started_at ON cron_run (started_at);
