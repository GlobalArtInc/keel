ALTER TABLE auth_session
    ADD COLUMN parent_id_hash TEXT REFERENCES auth_session(id_hash) ON DELETE RESTRICT,
    ADD COLUMN su_id_hash TEXT REFERENCES auth_session(id_hash) ON DELETE RESTRICT;

CREATE INDEX auth_session_parent_id_hash ON auth_session (parent_id_hash);
