use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
use std::sync::Arc;

#[derive(Default)]
pub struct Services {
    entries: HashMap<TypeId, (&'static str, Box<dyn Any + Send + Sync>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ServiceKey {
    id: TypeId,
    name: &'static str,
}

impl ServiceKey {
    pub fn of<T: ?Sized + 'static>() -> Self {
        Self {
            id: TypeId::of::<T>(),
            name: type_name::<T>(),
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl Services {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn provide<T: ?Sized + Send + Sync + 'static>(&mut self, service: Arc<T>) -> bool {
        let key = ServiceKey::of::<T>();
        self.entries
            .insert(key.id, (key.name, Box::new(service)))
            .is_none()
    }

    pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.entries
            .get(&TypeId::of::<T>())
            .and_then(|(_, service)| service.downcast_ref::<Arc<T>>())
            .cloned()
    }

    pub fn contains(&self, key: &ServiceKey) -> bool {
        self.entries.contains_key(&key.id)
    }

    pub fn keys(&self) -> impl Iterator<Item = ServiceKey> + '_ {
        self.entries
            .iter()
            .map(|(id, (name, _))| ServiceKey { id: *id, name })
    }

    pub fn merge(&mut self, other: Services) -> Vec<ServiceKey> {
        let mut conflicts = Vec::new();
        for (id, (name, service)) in other.entries {
            match self.entries.entry(id) {
                Entry::Occupied(_) => conflicts.push(ServiceKey { id, name }),
                Entry::Vacant(slot) => {
                    slot.insert((name, service));
                }
            }
        }
        conflicts
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl fmt::Debug for Services {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.entries.values().map(|(name, _)| name))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    trait Greeter: Send + Sync {
        fn greet(&self) -> String;
    }

    struct English;

    impl Greeter for English {
        fn greet(&self) -> String {
            "hello".into()
        }
    }

    #[test]
    fn services_store_and_return_trait_objects_by_type() {
        let mut services = Services::new();
        assert!(services.provide::<dyn Greeter>(Arc::new(English)));
        assert!(!services.provide::<dyn Greeter>(Arc::new(English)));
        let greeter = services.get::<dyn Greeter>().unwrap();
        assert_eq!(greeter.greet(), "hello");
        assert!(services.contains(&ServiceKey::of::<dyn Greeter>()));
        assert!(services.get::<String>().is_none());
        assert!(
            ServiceKey::of::<dyn Greeter>()
                .name()
                .ends_with("tests::Greeter")
        );
    }

    #[test]
    fn merging_services_reports_conflicts_without_replacing() {
        let mut first = Services::new();
        first.provide::<dyn Greeter>(Arc::new(English));
        let mut second = Services::new();
        second.provide::<dyn Greeter>(Arc::new(English));
        second.provide::<String>(Arc::new("x".to_owned()));
        let conflicts = first.merge(second);
        assert_eq!(conflicts, vec![ServiceKey::of::<dyn Greeter>()]);
        assert_eq!(first.get::<String>().as_deref(), Some(&"x".to_owned()));
        assert_eq!(format!("{first:?}").matches("Greeter").count(), 1);
    }
}
