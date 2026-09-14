use keel_sdk::{KeelError, Result};

const ROOT: &str = "svg";
const SCRIPT: &str = "script";

pub fn check_svg(text: &str) -> Result<()> {
    let document = roxmltree::Document::parse(text)
        .map_err(|error| refused().param("reason", error.to_string()))?;
    let root = document.root_element();
    if !root.tag_name().name().eq_ignore_ascii_case(ROOT) {
        return Err(refused().param("reason", "the root element is not svg"));
    }
    if document
        .descendants()
        .any(|node| node.is_element() && node.tag_name().name().eq_ignore_ascii_case(SCRIPT))
    {
        return Err(refused().param("reason", "a script element"));
    }
    Ok(())
}

fn refused() -> KeelError {
    KeelError::validation("settings.error.brand_svg").field("logo")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_svg_document_passes_while_scripts_other_roots_and_broken_xml_are_refused() {
        check_svg("<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"1\"/></svg>")
            .expect("a plain drawing");
        check_svg("<?xml version=\"1.0\"?>\n<SVG><g/></SVG>").expect("case is not significant");
        check_svg("<svg xmlns=\"http://www.w3.org/2000/svg\"><style>circle { fill: red }</style><circle r=\"1\"/></svg>")
            .expect("a style element is a drawing concern");
        for (svg, reason) in [
            (
                "<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>",
                "a script element",
            ),
            (
                "<svg xmlns=\"http://www.w3.org/2000/svg\"><g><SCRIPT href=\"x\"/></g></svg>",
                "a script element",
            ),
            ("<html><svg/></html>", "the root element is not svg"),
            ("<svg><circle>", ""),
            ("plain text", ""),
            ("", ""),
        ] {
            let error = check_svg(svg).expect_err(svg);
            assert_eq!(error.key(), "settings.error.brand_svg", "{svg}");
            assert_eq!(error.field_path(), Some("logo"), "{svg}");
            let actual = error.params()["reason"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if reason.is_empty() {
                assert!(!actual.is_empty(), "{svg}");
            } else {
                assert_eq!(actual, reason, "{svg}");
            }
        }
    }
}
