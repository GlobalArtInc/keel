use keel_sdk::types::is_mask;

pub const SUFFIX: &str = ".*";
pub const MAX_LEN: usize = 128;

pub fn is_valid(mask: &str) -> bool {
    mask.len() <= MAX_LEN && mask.contains('.') && is_mask(mask)
}

pub fn matches(mask: &str, kind: &str) -> bool {
    match mask.strip_suffix(SUFFIX) {
        Some(prefix) => kind
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.') && rest.len() > 1),
        None => mask == kind,
    }
}

pub fn any_matches<'a>(masks: impl IntoIterator<Item = &'a str>, kind: &str) -> bool {
    masks.into_iter().any(|mask| matches(mask, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mask_is_a_module_wildcard_or_a_dotted_name_and_matches_like_invalidate_on() {
        for valid in [
            "note.*",
            "dns.zone.*",
            "note.saved",
            "keel.account.created",
            "svc.*",
        ] {
            assert!(is_valid(valid), "{valid}");
        }
        for invalid in [
            "",
            "*",
            "note",
            ".*",
            "note.",
            "note..saved",
            "Note.*",
            "note.*.saved",
            "note saved",
            "note.sa-ved",
        ] {
            assert!(!is_valid(invalid), "{invalid}");
        }
        assert!(!is_valid(&format!("{}.*", "a".repeat(MAX_LEN))));
        assert!(matches("note.*", "note.saved"));
        assert!(matches("note.*", "note.tag.added"));
        assert!(matches("dns.zone.*", "dns.zone.created"));
        assert!(!matches("dns.zone.*", "dns.zones.created"));
        assert!(!matches("note.*", "note"));
        assert!(!matches("note.*", "note."));
        assert!(!matches("note.*", "notebook.saved"));
        assert!(matches("note.saved", "note.saved"));
        assert!(!matches("note.saved", "note.saved.twice"));
        assert!(any_matches(["dns.*", "note.saved"], "note.saved"));
        assert!(!any_matches(["dns.*", "note.deleted"], "note.saved"));
        assert!(!any_matches([], "note.saved"));
    }
}
