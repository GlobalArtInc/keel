use keel_sdk::prelude::*;

use super::super::api::{Cleared, Created, Message, Pair, Problem, Raise};
use super::super::direct::{self, Mailer, SendTo, Sent};
use super::super::{messages, problems};

pub const PROBLEM_RAISE: &str = "notify.problem.raise";
pub const PROBLEM_CLEAR: &str = "notify.problem.clear";
pub const MESSAGE_CREATE: &str = "notify.message.create";
pub const SEND_TO: &str = "notify.send_to";

pub fn problem_raise() -> FunctionDef {
    function::<Raise, Problem>(PROBLEM_RAISE)
        .kind(Kind::Action)
        .mode(Mode::Write)
        .level(Level::Admin)
        .unowned()
        .internal()
        .handler(|call: Call<Raise>| async move { problems::raise(&call).await })
}

pub fn problem_clear() -> FunctionDef {
    function::<Pair, Cleared>(PROBLEM_CLEAR)
        .kind(Kind::Action)
        .mode(Mode::Write)
        .level(Level::Admin)
        .unowned()
        .internal()
        .handler(|call: Call<Pair>| async move { problems::clear(&call).await })
}

pub fn message_create() -> FunctionDef {
    function::<Message, Created>(MESSAGE_CREATE)
        .kind(Kind::Action)
        .mode(Mode::Write)
        .level(Level::Admin)
        .unowned()
        .internal()
        .handler(|call: Call<Message>| async move { messages::create(&call).await })
}

pub fn send_to(mailer: Mailer) -> FunctionDef {
    function::<SendTo, Sent>(SEND_TO)
        .kind(Kind::Action)
        .mode(Mode::Write)
        .level(Level::Admin)
        .unowned()
        .internal()
        .handler(move |call: Call<SendTo>| {
            let mailer = mailer.clone();
            async move { direct::enqueue(&call, &mailer).await }
        })
}
