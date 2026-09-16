use serde::Serialize;

pub use keel_types::FileAttrs;

pub const PARAMS_PART: &str = "params";
pub const PARAMS_LIMIT: usize = 64 * 1024;
pub const BASE64_ENCODING: &str = "base64";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileField {
    pub file: FileAttrs,
}

pub fn file<I, S>(max_bytes: u64, types: I) -> FileField
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    FileField {
        file: FileAttrs::new(max_bytes, types),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_file_helper_writes_the_x_keel_member_the_kernel_reads() {
        let field = file(65536, ["image/svg+xml"]);
        assert_eq!(
            serde_json::to_value(&field).unwrap(),
            json!({ "file": { "maxBytes": 65536, "types": ["image/svg+xml"] } })
        );
        assert_eq!(field.file, FileAttrs::new(65536, vec!["image/svg+xml"]));
        assert_eq!(PARAMS_PART, "params");
        assert_eq!(PARAMS_LIMIT, 65536);
        assert_eq!(BASE64_ENCODING, "base64");
    }
}
