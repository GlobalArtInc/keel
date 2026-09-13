DROP INDEX auth_session_parent_id_hash;

ALTER TABLE auth_session
    DROP COLUMN su_id_hash,
    DROP COLUMN parent_id_hash;
