use std::collections::BTreeMap;

const DEFAULT_LANGUAGE: &str = "en";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailText {
    pub subject: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailTemplate {
    id: String,
    texts: BTreeMap<String, MailText>,
}

impl MailTemplate {
    pub fn new(id: &str, subject: &str, body: &str) -> Self {
        Self {
            id: id.to_owned(),
            texts: BTreeMap::new(),
        }
        .lang(DEFAULT_LANGUAGE, subject, body)
    }

    pub fn lang(mut self, lang: &str, subject: &str, body: &str) -> Self {
        self.texts.insert(
            lang.to_owned(),
            MailText {
                subject: subject.to_owned(),
                body: body.to_owned(),
            },
        );
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn module(&self) -> &str {
        self.id
            .split_once('.')
            .map_or(&self.id, |(module, _)| module)
    }

    pub fn languages(&self) -> impl Iterator<Item = &str> {
        self.texts.keys().map(String::as_str)
    }

    pub fn text(&self, lang: &str) -> Option<&MailText> {
        self.texts.get(lang)
    }

    pub fn texts(&self) -> impl Iterator<Item = (&str, &MailText)> {
        self.texts.iter().map(|(lang, text)| (lang.as_str(), text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_starts_in_english_and_gathers_its_languages() {
        let template = MailTemplate::new("probe.hello", "Hi {name}", "body").lang(
            "ru",
            "Hi in ru {name}",
            "ru body",
        );
        assert_eq!(template.id(), "probe.hello");
        assert_eq!(template.module(), "probe");
        assert_eq!(template.languages().collect::<Vec<_>>(), ["en", "ru"]);
        assert_eq!(
            template.text("ru").map(|text| text.body.as_str()),
            Some("ru body")
        );
        assert_eq!(
            template
                .texts()
                .map(|(lang, text)| (lang, text.subject.as_str()))
                .collect::<Vec<_>>(),
            [("en", "Hi {name}"), ("ru", "Hi in ru {name}")]
        );
        assert_eq!(MailTemplate::new("bare", "s", "b").module(), "bare");
    }
}
