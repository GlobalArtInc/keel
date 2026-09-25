use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

pub(crate) fn write_tool(path: &Path, body: &str) {
    let mut tee = Command::new("tee")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    tee.stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    assert!(tee.wait().unwrap().success());
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
