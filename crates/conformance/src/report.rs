use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Failure(pub String);

impl From<reqwest::Error> for Failure {
    fn from(error: reqwest::Error) -> Self {
        Failure(format!("the request failed: {error}"))
    }
}

impl From<serde_json::Error> for Failure {
    fn from(error: serde_json::Error) -> Self {
        Failure(format!("the body does not fit the expected shape: {error}"))
    }
}

pub fn fail(message: impl Into<String>) -> Failure {
    Failure(message.into())
}

pub fn ensure(condition: bool, message: impl FnOnce() -> String) -> Result<(), Failure> {
    if condition {
        Ok(())
    } else {
        Err(Failure(message()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aborted;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub passed: u32,
    pub failed: u32,
    pub aborted: u32,
}

impl Outcome {
    pub fn is_green(&self) -> bool {
        self.failed == 0 && self.aborted == 0
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} checks, {} passed, {} failed, {} groups aborted",
            self.passed + self.failed,
            self.passed,
            self.failed,
            self.aborted
        )
    }
}

#[derive(Debug, Default)]
pub struct Report {
    outcome: Outcome,
}

impl Report {
    pub fn check<T>(&mut self, name: &str, outcome: Result<T, Failure>) -> Result<T, Aborted> {
        match outcome {
            Ok(value) => {
                self.outcome.passed += 1;
                println!("ok: {name}");
                Ok(value)
            }
            Err(failure) => {
                self.outcome.failed += 1;
                println!("FAILED: {name}: {failure}");
                Err(Aborted)
            }
        }
    }

    pub fn aborted(&mut self, group: &str) {
        self.outcome.aborted += 1;
        println!("aborted: the {group} checks stop at the first failure");
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_check_aborts_its_group_and_turns_the_outcome_red() {
        let mut report = Report::default();
        assert_eq!(report.check("one", Ok::<_, Failure>(1)), Ok(1));
        assert!(report.outcome().is_green());
        assert_eq!(report.check::<()>("two", Err(fail("wrong"))), Err(Aborted));
        report.aborted("group");
        let outcome = report.outcome();
        assert!(!outcome.is_green());
        assert_eq!(
            outcome.to_string(),
            "2 checks, 1 passed, 1 failed, 1 groups aborted"
        );
    }

    #[test]
    fn ensure_answers_the_message_only_when_the_condition_fails() {
        assert_eq!(ensure(true, || "unused".into()), Ok(()));
        assert_eq!(ensure(false, || "wrong".into()), Err(fail("wrong")));
    }
}
