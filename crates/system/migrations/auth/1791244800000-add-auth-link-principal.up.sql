ALTER TABLE auth_token
    ADD COLUMN link_principal_id TEXT;

ALTER TABLE auth_login_key
    ADD COLUMN link_principal_id TEXT;

ALTER TABLE auth_session
    ADD COLUMN link_principal_id TEXT;

CREATE INDEX auth_token_link_principal_id ON auth_token (link_principal_id) WHERE link_principal_id IS NOT NULL;
CREATE INDEX auth_login_key_link_principal_id ON auth_login_key (link_principal_id) WHERE link_principal_id IS NOT NULL;
CREATE INDEX auth_session_link_principal_id ON auth_session (link_principal_id) WHERE link_principal_id IS NOT NULL;
