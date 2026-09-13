use axum::http::HeaderValue;
use axum::http::header::SET_COOKIE;
use axum::response::Response;
use keel_sdk::system::SessionChange;

use super::auth::SESSION_COOKIE;

const ATTRIBUTES: &str = "Path=/; Secure; HttpOnly; SameSite=Lax";
const CLEARED: &str = "Max-Age=0";

pub(super) fn header(change: &SessionChange) -> String {
    match change {
        SessionChange::Set(token) => format!("{SESSION_COOKIE}={}; {ATTRIBUTES}", token.expose()),
        SessionChange::Clear => format!("{SESSION_COOKIE}=; {ATTRIBUTES}; {CLEARED}"),
    }
}

pub(super) fn apply(response: &mut Response, change: &SessionChange) {
    match HeaderValue::from_str(&header(change)) {
        Ok(value) => {
            response.headers_mut().insert(SET_COOKIE, value);
        }
        Err(error) => tracing::error!(error = %error, "the session cookie is not a header value"),
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::Secret;

    use super::*;

    #[test]
    fn a_set_carries_the_token_without_an_age_and_a_clear_expires_the_same_cookie() {
        assert_eq!(
            header(&SessionChange::Set(Secret::new("ks_abc"))),
            "__Host-keel_session=ks_abc; Path=/; Secure; HttpOnly; SameSite=Lax"
        );
        assert_eq!(
            header(&SessionChange::Clear),
            "__Host-keel_session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0"
        );
    }
}
