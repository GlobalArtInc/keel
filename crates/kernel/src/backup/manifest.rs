use semver::Version;

use super::BackupError;

const COMPATIBLE: [(u64, &[u64]); 1] = [(0, &[0])];

pub fn compatible(archive: &str, running: &str) -> Result<(), BackupError> {
    let incompatible = |reason: String| BackupError::Incompatible {
        archive: archive.to_owned(),
        running: running.to_owned(),
        reason,
    };
    let taken = Version::parse(archive)
        .map_err(|error| incompatible(format!("the archive version does not parse: {error}")))?;
    let current = Version::parse(running)
        .map_err(|error| incompatible(format!("the running version does not parse: {error}")))?;
    let admitted = COMPATIBLE
        .iter()
        .find(|(major, _)| *major == current.major)
        .is_some_and(|(_, majors)| majors.contains(&taken.major));
    if !admitted {
        return Err(incompatible(format!(
            "a keel {} binary restores archives of major {} only",
            current.major,
            COMPATIBLE
                .iter()
                .find(|(major, _)| *major == current.major)
                .map(|(_, majors)| {
                    majors
                        .iter()
                        .map(u64::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "none".to_owned())
        )));
    }
    if taken > current {
        return Err(incompatible(
            "the archive is newer than the binary; its tables may carry columns this binary does not know"
                .to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_admits_the_same_major_up_to_the_running_version() {
        compatible("0.1.0", "0.1.0").unwrap();
        compatible("0.1.0", "0.2.3").unwrap();
        compatible("0.1.0-beta.1", "0.1.0").unwrap();
        let newer = compatible("0.2.0", "0.1.0").unwrap_err();
        assert!(
            newer.to_string().contains("newer than the binary"),
            "{newer}"
        );
        let major = compatible("1.0.0", "0.1.0").unwrap_err();
        assert!(
            major
                .to_string()
                .contains("a keel 0 binary restores archives of major 0 only"),
            "{major}"
        );
        let unknown = compatible("2.0.0", "3.0.0").unwrap_err();
        assert!(unknown.to_string().contains("major none only"), "{unknown}");
        let garbage = compatible("yesterday", "0.1.0").unwrap_err();
        assert!(
            matches!(garbage, BackupError::Incompatible { .. }),
            "{garbage}"
        );
        assert!(compatible("0.1.0", "soon").is_err());
    }
}
