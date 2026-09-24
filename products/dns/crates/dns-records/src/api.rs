use keel::prelude::*;

pub use crate::value::RecordType;

pub fn check(
    kind: RecordType,
    name: &str,
    value: &str,
    priority: Option<u16>,
    weight: Option<u16>,
    port: Option<u16>,
) -> Result<()> {
    crate::value::Entry::checked(kind, name, value, priority, weight, port).map(drop)
}
