DROP INDEX auth_session_link_principal_id;
DROP INDEX auth_login_key_link_principal_id;
DROP INDEX auth_token_link_principal_id;

ALTER TABLE auth_session
    DROP COLUMN link_principal_id;

ALTER TABLE auth_login_key
    DROP COLUMN link_principal_id;

ALTER TABLE auth_token
    DROP COLUMN link_principal_id;
