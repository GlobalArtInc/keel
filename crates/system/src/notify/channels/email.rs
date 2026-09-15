use std::time::Duration;

use keel_sdk::{KeelError, Result, Secret};
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use super::{Security, StoredEmail};

pub const TIMEOUT: Duration = Duration::from_secs(30);
pub const ERROR_ADDRESS: &str = "notify.error.email_address";
pub const ERROR_TRANSPORT: &str = "notify.error.email_transport";
pub const ERROR_SEND: &str = "notify.error.email_send";

pub struct Mail<'a> {
    pub subject: &'a str,
    pub body: &'a str,
}

pub fn transport(
    email: &StoredEmail,
    password: Option<Secret>,
) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let transport = |error: lettre::transport::smtp::Error| {
        KeelError::unavailable(ERROR_TRANSPORT)
            .param("host", email.host.as_str())
            .param("reason", error.to_string())
            .source(error)
    };
    let mut builder = match email.security {
        Security::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&email.host),
        Security::Starttls => {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&email.host).map_err(transport)?
        }
        Security::Tls => {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&email.host).map_err(transport)?
        }
    }
    .port(email.port)
    .timeout(Some(TIMEOUT));
    if let Some(username) = &email.username {
        let password = password.map(Secret::into_inner).unwrap_or_default();
        builder = builder.credentials(Credentials::new(username.clone(), password));
    }
    Ok(builder.build())
}

pub fn message(email: &StoredEmail, mail: &Mail<'_>) -> Result<Message> {
    let address = |field: &str, value: &str| {
        value.parse::<lettre::message::Mailbox>().map_err(|error| {
            KeelError::validation(ERROR_ADDRESS)
                .field(format!("email.{field}"))
                .param("value", value)
                .source(error)
        })
    };
    Message::builder()
        .from(address("from", &email.from)?)
        .to(address("to", &email.to)?)
        .subject(mail.subject)
        .header(ContentType::TEXT_PLAIN)
        .body(mail.body.to_owned())
        .map_err(|error| KeelError::internal(ERROR_SEND).source(error))
}

pub async fn send(email: &StoredEmail, password: Option<Secret>, mail: &Mail<'_>) -> Result<()> {
    let transport = transport(email, password)?;
    let message = message(email, mail)?;
    transport.send(message).await.map_err(|error| {
        KeelError::unavailable(ERROR_SEND)
            .param("host", email.host.as_str())
            .param("reason", error.to_string())
            .source(error)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email(security: Security) -> StoredEmail {
        StoredEmail {
            host: "mail.example.org".into(),
            port: 2525,
            security,
            username: Some("keel".into()),
            password: None,
            from: "Keel <keel@example.org>".into(),
            to: "ops@example.org".into(),
        }
    }

    #[test]
    fn a_transport_builds_for_every_security_and_a_bad_address_names_its_field() {
        for security in Security::ALL {
            transport(&email(security), Some(Secret::new("pw"))).expect("the transport builds");
        }
        let mail = Mail {
            subject: "Hello",
            body: "World",
        };
        message(&email(Security::None), &mail).expect("the message builds");
        let mut broken = email(Security::None);
        broken.to = "not an address".into();
        let error = message(&broken, &mail).expect_err("the address is refused");
        assert_eq!(error.key(), ERROR_ADDRESS);
        assert_eq!(error.field_path(), Some("email.to"));
    }
}
