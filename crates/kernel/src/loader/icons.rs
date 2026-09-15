use std::collections::BTreeSet;

pub(super) const ICONS_FILE: &str = "contracts/icons.json";

const ICONS: &str = include_str!("../../../../contracts/icons.json");

#[derive(Debug)]
pub(super) struct IconSet(Option<BTreeSet<&'static str>>);

impl IconSet {
    pub(super) fn embedded() -> Result<Self, serde_json::Error> {
        let names: Vec<&'static str> = serde_json::from_str(ICONS)?;
        Ok(Self(Some(names.into_iter().collect())))
    }

    pub(super) fn any() -> Self {
        Self(None)
    }

    pub(super) fn admits(&self, icon: &str) -> bool {
        self.0.as_ref().is_none_or(|names| names.contains(icon))
    }

    #[cfg(test)]
    pub(super) fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.0.iter().flatten().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_icon_set_is_sorted_unique_and_names_plus() {
        let listed: Vec<&str> = serde_json::from_str(ICONS).expect("the icon file is a JSON array");
        let mut sorted = listed.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(listed, sorted, "{ICONS_FILE} is sorted without repeats");
        let icons = IconSet::embedded().expect("the icon file parses");
        assert_eq!(icons.names().count(), listed.len());
        assert!(icons.names().count() > 200);
        assert!(icons.admits("plus"));
        assert!(icons.admits("settings"));
        assert!(!icons.admits("Plus"));
        assert!(!icons.admits("flask"));
        assert!(IconSet::any().admits("flask"));
    }
}
