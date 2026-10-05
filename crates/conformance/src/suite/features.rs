use keel_types::{Boot, BootProduct, Descriptor, ErrorCode, LicenseState};
use serde_json::json;

use super::Suite;
use crate::client::{Answer, Credential};
use crate::report::{Aborted, Failure, ensure};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Withheld {
    pub product: &'static str,
    pub edition: &'static str,
    pub function: &'static str,
    pub feature: &'static str,
}

pub const WITHHELD: [Withheld; 2] = [
    Withheld {
        product: "demo",
        edition: "full",
        function: "note.withheld",
        feature: "demo.withheld",
    },
    Withheld {
        product: "dns",
        edition: "lite",
        function: "dns.zone.dnssec.get",
        feature: "dns.dnssec",
    },
];

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let outcome = suite.login().await;
    suite
        .report
        .check("auth.login as the admin for the feature checks", outcome)?;
    let answer = suite
        .client
        .get("/api/v1/boot", Credential::Session, &[])
        .await;
    let boot = suite.report.check(
        "GET /api/v1/boot names the products and the features the installation enables",
        answer.and_then(|answer| {
            answer.status_is(200)?;
            answer.body_as::<Boot>()
        }),
    )?;
    let answer = suite
        .client
        .get("/api/v1/functions", Credential::Session, &[])
        .await;
    let listing = suite.report.check(
        "every function of the descriptor listing names no feature beyond the enabled ones",
        answer.and_then(|answer| enabled_only(&answer, &boot.features)),
    )?;
    let rows = suite.report.check(
        "every enabled product of the suite's table that runs on the edition withholding its feature, as each does while no lease is installed, leaves that feature out of the boot",
        applicable(
            &boot.products,
            &boot.features,
            boot.license.as_ref().map(|license| license.state),
        ),
    )?;
    for withheld in rows {
        let function = withheld.function;
        let feature = withheld.feature;
        let answer = suite
            .client
            .call(function, json!({}), Credential::Session, &[])
            .await;
        suite.report.check(
            &format!(
                "{function} behind {feature}, which the edition leaves out, answers 402 license keel.error.license naming the feature"
            ),
            unlicensed(answer, feature),
        )?;
        let answer = suite
            .client
            .get(
                &format!("/api/v1/functions/{function}"),
                Credential::Session,
                &[],
            )
            .await;
        suite.report.check(
            &format!("the descriptor of {function} answers 402 license as well"),
            unlicensed(answer, feature),
        )?;
        suite.report.check(
            &format!("the descriptor listing leaves {function} out"),
            ensure(
                !listing.iter().any(|descriptor| descriptor.name == function),
                || format!("the listing carries {function}"),
            ),
        )?;
    }
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout after the feature checks", outcome)?;
    Ok(())
}

pub fn applicable(
    products: &[BootProduct],
    features: &[String],
    license: Option<LicenseState>,
) -> Result<Vec<Withheld>, Failure> {
    let Some(state) = license else {
        return Ok(Vec::new());
    };
    let leased = state != LicenseState::None;
    let mut rows = Vec::new();
    for withheld in WITHHELD {
        let Some(product) = products
            .iter()
            .find(|product| product.id == withheld.product)
        else {
            continue;
        };
        let edition = product.edition.as_deref();
        if leased && edition != Some(withheld.edition) {
            continue;
        }
        ensure(edition == Some(withheld.edition), || {
            format!(
                "{} runs on the edition {edition:?} while no lease is installed, not on {}, its default",
                withheld.product, withheld.edition
            )
        })?;
        ensure(
            !features.iter().any(|feature| feature == withheld.feature),
            || {
                format!(
                    "the edition {} of {} grants {}, which it withholds",
                    withheld.edition, withheld.product, withheld.feature
                )
            },
        )?;
        rows.push(withheld);
    }
    Ok(rows)
}

fn enabled_only(answer: &Answer, features: &[String]) -> Result<Vec<Descriptor>, Failure> {
    answer.status_is(200)?;
    let descriptors: Vec<Descriptor> = answer.body_as()?;
    let beyond: Vec<String> = descriptors
        .iter()
        .filter_map(|descriptor| {
            descriptor
                .access
                .feature
                .as_ref()
                .filter(|feature| !features.contains(feature))
                .map(|feature| format!("{} behind {feature}", descriptor.name))
        })
        .collect();
    ensure(beyond.is_empty(), || {
        answer
            .failure(format!(
                "the listing serves {beyond:?} while the boot enables {features:?}"
            ))
            .0
    })?;
    Ok(descriptors)
}

fn unlicensed(answer: Result<Answer, Failure>, feature: &str) -> Result<(), Failure> {
    let answer = answer?;
    let error = answer.error(402, ErrorCode::License, "keel.error.license")?;
    ensure(error.params["feature"] == feature, || {
        answer.failure("params.feature does not name the feature").0
    })
}

#[cfg(test)]
mod tests {
    use keel_types::BootBrand;

    use super::*;

    fn on(id: &str, edition: &str) -> BootProduct {
        BootProduct {
            id: id.to_owned(),
            name: format!("product.{id}.brand"),
            edition: Some(edition.to_owned()),
            brand: BootBrand {
                name: id.to_owned(),
                logo: None,
                hue: 0,
                chroma: 0.0,
            },
        }
    }

    fn names(
        products: &[BootProduct],
        features: &[&str],
        license: Option<LicenseState>,
    ) -> Result<Vec<&'static str>, Failure> {
        let features: Vec<String> = features.iter().map(|&feature| feature.to_owned()).collect();
        applicable(products, &features, license)
            .map(|rows| rows.iter().map(|withheld| withheld.function).collect())
    }

    #[test]
    fn a_withheld_function_applies_to_an_enabled_product_on_the_edition_that_withholds_it() {
        let unleased = Some(LicenseState::None);
        let lite = [on("dns", "lite")];
        assert_eq!(
            names(&lite, &["dns.zones"], unleased).unwrap(),
            ["dns.zone.dnssec.get"]
        );
        let both = [on("dns", "lite"), on("panel", "lite")];
        assert_eq!(
            names(&both, &["dns.zones", "panel.web"], unleased).unwrap(),
            ["dns.zone.dnssec.get"]
        );
        assert!(
            names(&[on("panel", "lite")], &["panel.web"], unleased)
                .unwrap()
                .is_empty()
        );
        let all = [on("demo", "full"), on("dns", "lite")];
        assert_eq!(
            names(&all, &["dns.zones"], unleased).unwrap(),
            ["note.withheld", "dns.zone.dnssec.get"]
        );
        for state in [LicenseState::Valid, LicenseState::Grace] {
            assert_eq!(
                names(&lite, &["dns.zones"], Some(state)).unwrap(),
                ["dns.zone.dnssec.get"]
            );
            assert!(
                names(
                    &[on("dns", "pro")],
                    &["dns.zones", "dns.dnssec"],
                    Some(state)
                )
                .unwrap()
                .is_empty()
            );
        }
    }

    #[test]
    fn an_edition_that_grants_what_it_withholds_fails_instead_of_dropping_the_check() {
        let lite = [on("dns", "lite")];
        let granted = ["dns.zones", "dns.dnssec"];
        let refused = names(&lite, &granted, Some(LicenseState::None)).unwrap_err();
        assert!(refused.0.contains("grants dns.dnssec"), "{refused}");
        assert!(names(&lite, &granted, Some(LicenseState::Valid)).is_err());
        let moved = names(&[on("dns", "pro")], &granted, Some(LicenseState::None)).unwrap_err();
        assert!(moved.0.contains("no lease"), "{moved}");
        assert!(names(&lite, &granted, None).unwrap().is_empty());
    }
}
