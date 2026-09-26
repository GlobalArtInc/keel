use std::collections::BTreeMap;
use std::fmt;

use keel_sdk::serde_json::Value;
use keel_sdk::{KeelError, MailTemplate, Result};
use minijinja::Environment;
use minijinja::syntax::SyntaxConfig;

pub const DEFAULT_LANGUAGE: &str = "en";
pub const ERROR_TWICE: &str = "notify.error.template_twice";
pub const ERROR_SYNTAX: &str = "notify.error.template_syntax";
pub const ERROR_PLACEHOLDER: &str = "notify.error.template_placeholder";
pub const ERROR_MISSING: &str = "notify.error.template_missing";
pub const ERROR_RENDER: &str = "notify.error.template_render";
pub const VARIABLE_DELIMITERS: (&str, &str) = ("{", "}");
pub const BLOCK_DELIMITERS: (&str, &str) = ("{%", "%}");
pub const COMMENT_DELIMITERS: (&str, &str) = ("{#", "#}");

const FORBIDDEN: [&str; 3] = ["{{", "}}", "%s"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    pub subject: String,
    pub body: String,
    pub lang: String,
}

pub struct Templates {
    env: Environment<'static>,
    languages: BTreeMap<String, Vec<String>>,
}

fn syntax() -> Result<SyntaxConfig> {
    SyntaxConfig::builder()
        .variable_delimiters(VARIABLE_DELIMITERS.0, VARIABLE_DELIMITERS.1)
        .block_delimiters(BLOCK_DELIMITERS.0, BLOCK_DELIMITERS.1)
        .comment_delimiters(COMMENT_DELIMITERS.0, COMMENT_DELIMITERS.1)
        .build()
        .map_err(|error| KeelError::internal(ERROR_SYNTAX).source(error))
}

fn name(id: &str, lang: &str, part: &str) -> String {
    format!("{id}/{lang}/{part}")
}

pub fn placeholder_fault(text: &str) -> Option<&'static str> {
    FORBIDDEN
        .into_iter()
        .find(|fragment| text.contains(fragment))
}

impl Templates {
    pub fn compile<'a>(contributed: impl IntoIterator<Item = &'a MailTemplate>) -> Result<Self> {
        let mut env = Environment::new();
        env.set_syntax(syntax()?);
        let mut languages: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for template in contributed {
            if languages.contains_key(template.id()) {
                return Err(KeelError::internal(ERROR_TWICE).param("id", template.id()));
            }
            let mut langs = Vec::new();
            for (lang, text) in template.texts() {
                for (part, source) in [("subject", &text.subject), ("body", &text.body)] {
                    if let Some(found) = placeholder_fault(source) {
                        return Err(KeelError::internal(ERROR_PLACEHOLDER)
                            .param("id", template.id())
                            .param("lang", lang)
                            .param("part", part)
                            .param("found", found));
                    }
                    env.add_template_owned(name(template.id(), lang, part), source.clone())
                        .map_err(|error| {
                            KeelError::internal(ERROR_SYNTAX)
                                .param("id", template.id())
                                .param("lang", lang)
                                .param("part", part)
                                .param("reason", error.to_string())
                                .source(error)
                        })?;
                }
                langs.push(lang.to_owned());
            }
            languages.insert(template.id().to_owned(), langs);
        }
        Ok(Self { env, languages })
    }

    pub fn has(&self, id: &str) -> bool {
        self.languages.contains_key(id)
    }

    #[cfg(test)]
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.languages.keys().map(String::as_str)
    }

    fn language_of(&self, id: &str, lang: &str) -> Result<String> {
        let languages = self
            .languages
            .get(id)
            .ok_or_else(|| KeelError::internal(ERROR_MISSING).param("id", id))?;
        languages
            .iter()
            .find(|known| *known == lang)
            .or_else(|| languages.iter().find(|known| *known == DEFAULT_LANGUAGE))
            .or_else(|| languages.first())
            .cloned()
            .ok_or_else(|| KeelError::internal(ERROR_MISSING).param("id", id))
    }

    pub fn render(&self, id: &str, lang: &str, context: &Value) -> Result<Rendered> {
        let lang = self.language_of(id, lang)?;
        let mut parts = Vec::with_capacity(2);
        for part in ["subject", "body"] {
            let template = self
                .env
                .get_template(&name(id, &lang, part))
                .map_err(|error| {
                    KeelError::internal(ERROR_MISSING)
                        .param("id", id)
                        .source(error)
                })?;
            let text = template.render(context).map_err(|error| {
                KeelError::internal(ERROR_RENDER)
                    .param("id", id)
                    .param("lang", lang.as_str())
                    .param("part", part)
                    .param("reason", error.to_string())
                    .source(error)
            })?;
            parts.push(text.trim_end_matches('\n').to_owned());
        }
        let body = parts.pop().unwrap_or_default();
        let subject = parts.pop().unwrap_or_default();
        Ok(Rendered {
            subject,
            body,
            lang,
        })
    }
}

impl fmt::Debug for Templates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Templates")
            .field("ids", &self.languages)
            .finish()
    }
}

pub fn shipped() -> [MailTemplate; 3] {
    [
        MailTemplate::new(
            super::api::KEY_PROBLEM_RAISED,
            include_str!("../../templates/notify/problem_raised.en.subject.txt"),
            include_str!("../../templates/notify/problem_raised.en.body.txt"),
        )
        .lang(
            "ru",
            include_str!("../../templates/notify/problem_raised.ru.subject.txt"),
            include_str!("../../templates/notify/problem_raised.ru.body.txt"),
        ),
        MailTemplate::new(
            super::api::KEY_PROBLEM_CLEARED,
            include_str!("../../templates/notify/problem_cleared.en.subject.txt"),
            include_str!("../../templates/notify/problem_cleared.en.body.txt"),
        )
        .lang(
            "ru",
            include_str!("../../templates/notify/problem_cleared.ru.subject.txt"),
            include_str!("../../templates/notify/problem_cleared.ru.body.txt"),
        ),
        MailTemplate::new(
            super::api::KEY_TEST,
            include_str!("../../templates/notify/test.en.subject.txt"),
            include_str!("../../templates/notify/test.en.body.txt"),
        )
        .lang(
            "ru",
            include_str!("../../templates/notify/test.ru.subject.txt"),
            include_str!("../../templates/notify/test.ru.body.txt"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::json;

    use super::*;

    #[test]
    fn a_template_renders_single_brace_placeholders_and_falls_back_to_english() {
        let templates = Templates::compile(&shipped()).expect("the shipped templates compile");
        let context = json!({
            "key": "notify.problem.raised",
            "level": "warning",
            "principal": "admin",
            "params": { "key": "note.overdue", "subject": "a1", "severity": "warning", "source": "note.overdue", "count": 3, "raisedAt": "2026-09-15T10:00:00Z" }
        });
        let rendered = templates
            .render(super::super::api::KEY_PROBLEM_RAISED, "en", &context)
            .expect("renders");
        assert_eq!(rendered.subject, "Problem note.overdue raised for a1");
        assert!(
            rendered.body.contains("Seen:     3 times"),
            "{}",
            rendered.body
        );
        assert_eq!(rendered.lang, "en");
        let russian = templates
            .render(super::super::api::KEY_PROBLEM_RAISED, "ru", &context)
            .expect("renders");
        assert_eq!(russian.lang, "ru");
        assert_ne!(russian.subject, rendered.subject);
        let shipped_ru = shipped();
        let ru_subject = &shipped_ru[0].text("ru").expect("a ru text").subject;
        let opening = ru_subject.split('{').next().unwrap_or_default();
        assert!(!opening.is_empty(), "{ru_subject}");
        assert!(russian.subject.starts_with(opening), "{}", russian.subject);
        assert!(
            russian.subject.contains("note.overdue"),
            "{}",
            russian.subject
        );
        assert!(russian.subject.ends_with("a1"), "{}", russian.subject);
        let fallback = templates
            .render(super::super::api::KEY_PROBLEM_RAISED, "de", &context)
            .expect("renders");
        assert_eq!(fallback.lang, "en");
        let once = json!({ "params": { "key": "k", "subject": "s", "severity": "warning", "source": "x", "count": 1, "raisedAt": "now" } });
        let rendered = templates
            .render(super::super::api::KEY_PROBLEM_RAISED, "en", &once)
            .expect("renders");
        assert!(
            rendered.body.contains("Seen:     1 time\n"),
            "{}",
            rendered.body
        );
        let missing = templates
            .render("notify.nothing", "en", &context)
            .expect_err("no such template");
        assert_eq!(missing.key(), ERROR_MISSING);
        assert!(templates.has(super::super::api::KEY_TEST));
        assert_eq!(templates.ids().count(), 3);
        assert!(format!("{templates:?}").starts_with("Templates { ids:"));
    }

    #[test]
    fn a_double_brace_a_twice_contributed_id_and_a_syntax_error_are_refused() {
        let doubled = MailTemplate::new("probe.hello", "Hi {{name}}", "body");
        let error = Templates::compile([&doubled]).expect_err("double braces are refused");
        assert_eq!(error.key(), ERROR_PLACEHOLDER);
        assert_eq!(error.params()["id"], json!("probe.hello"));
        assert_eq!(error.params()["part"], json!("subject"));
        assert_eq!(error.params()["found"], json!("{{"));
        let printf = MailTemplate::new("probe.hello", "Hi", "Hello %s");
        assert_eq!(
            Templates::compile([&printf])
                .expect_err("printf placeholders are refused")
                .params()["found"],
            json!("%s")
        );
        let first = MailTemplate::new("probe.hello", "Hi {name}", "body");
        let second = MailTemplate::new("probe.hello", "Hello", "body");
        let error = Templates::compile([&first, &second]).expect_err("the id collides");
        assert_eq!(error.key(), ERROR_TWICE);
        assert_eq!(error.params()["id"], json!("probe.hello"));
        let broken = MailTemplate::new("probe.hello", "Hi {name", "body");
        let error =
            Templates::compile([&broken]).expect_err("an unbalanced brace is a syntax error");
        assert_eq!(error.key(), ERROR_SYNTAX);
        assert_eq!(error.params()["part"], json!("subject"));
        assert_eq!(placeholder_fault("100% fine {name}"), None);
        assert_eq!(placeholder_fault("{ name }"), None);
        assert_eq!(doubled.module(), "probe");
        assert_eq!(doubled.languages().collect::<Vec<_>>(), ["en"]);
        assert_eq!(
            doubled.text("en").map(|text| text.body.as_str()),
            Some("body")
        );
    }
}
