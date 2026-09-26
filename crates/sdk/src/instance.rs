#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstanceInfoSource {
    features: Vec<String>,
    service_kinds: Vec<String>,
}

impl InstanceInfoSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feature(mut self, name: &str) -> Self {
        self.features.push(name.to_owned());
        self
    }

    pub fn service_kind(mut self, kind: &str) -> Self {
        self.service_kinds.push(kind.to_owned());
        self
    }

    pub fn features(&self) -> &[String] {
        &self.features
    }

    pub fn service_kinds(&self) -> &[String] {
        &self.service_kinds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_keeps_its_features_and_service_kinds_in_the_order_given() {
        let source = InstanceInfoSource::new()
            .feature("peers")
            .feature("events")
            .service_kind("dns.zones");
        assert_eq!(source.features(), ["peers", "events"]);
        assert_eq!(source.service_kinds(), ["dns.zones"]);
        assert_eq!(InstanceInfoSource::new(), InstanceInfoSource::default());
    }
}
