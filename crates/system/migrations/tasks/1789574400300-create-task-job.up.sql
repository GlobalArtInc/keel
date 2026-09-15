CREATE TABLE task_job (
    id                 TEXT PRIMARY KEY,
    class              TEXT NOT NULL,
    function           TEXT NOT NULL,
    params_json        JSONB NOT NULL,
    params_masked_json JSONB NOT NULL,
    principal_id       TEXT NOT NULL,
    principal_json     JSONB NOT NULL,
    account_id         TEXT NOT NULL,
    state              TEXT NOT NULL,
    resumable          BOOLEAN NOT NULL,
    progress_done      BIGINT NOT NULL DEFAULT 0,
    progress_total     BIGINT,
    message            TEXT,
    checkpoint_json    JSONB,
    result_json        JSONB,
    error_json         JSONB,
    request_id         TEXT,
    created_at         TIMESTAMPTZ NOT NULL,
    started_at         TIMESTAMPTZ,
    finished_at        TIMESTAMPTZ
);

CREATE INDEX task_job_state_class_created_at ON task_job (state, class, created_at);
CREATE INDEX task_job_account_id_created_at ON task_job (account_id, created_at);

CREATE TABLE task_step (
    job_id          TEXT NOT NULL REFERENCES task_job (id) ON DELETE CASCADE,
    seq             BIGINT NOT NULL,
    at              TIMESTAMPTZ NOT NULL,
    done            BIGINT NOT NULL,
    total           BIGINT,
    message         TEXT,
    checkpoint_json JSONB,
    PRIMARY KEY (job_id, seq)
);
