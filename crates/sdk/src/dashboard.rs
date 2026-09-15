use keel_types::{Level, WidgetKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DashboardWidget {
    kind: WidgetKind,
    func: String,
    limit: Option<u32>,
    level: Option<Level>,
}

impl DashboardWidget {
    pub fn new(kind: WidgetKind, func: &str) -> Self {
        Self {
            kind,
            func: func.to_owned(),
            limit: None,
            level: None,
        }
    }

    pub fn list(func: &str, limit: u32) -> Self {
        Self::new(WidgetKind::List, func).limit(limit)
    }

    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn level(mut self, level: Level) -> Self {
        self.level = Some(level);
        self
    }

    pub fn kind(&self) -> WidgetKind {
        self.kind
    }

    pub fn func(&self) -> &str {
        &self.func
    }

    pub fn limit_value(&self) -> Option<u32> {
        self.limit
    }

    pub fn level_value(&self) -> Option<Level> {
        self.level
    }

    pub fn module(&self) -> &str {
        self.func
            .split_once('.')
            .map_or(&self.func, |(module, _)| module)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_widget_names_its_kind_function_and_optional_limit() {
        let bare = DashboardWidget::new(WidgetKind::Stat, "dns.widget.zones");
        assert_eq!(bare.kind(), WidgetKind::Stat);
        assert_eq!(bare.func(), "dns.widget.zones");
        assert_eq!(bare.limit_value(), None);
        assert_eq!(bare.level_value(), None);
        assert_eq!(bare.module(), "dns");
        assert_eq!(
            DashboardWidget::list("notify.problem.list", 5)
                .level(Level::Admin)
                .level_value(),
            Some(Level::Admin)
        );
        let listed = DashboardWidget::list("notify.problem.list", 5);
        assert_eq!(listed.kind(), WidgetKind::List);
        assert_eq!(listed.limit_value(), Some(5));
        assert_eq!(
            listed,
            DashboardWidget::new(WidgetKind::List, "notify.problem.list").limit(5)
        );
    }
}
