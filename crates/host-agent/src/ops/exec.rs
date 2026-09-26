use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{OpenOptionsExt, fchown};
use std::path::Path;

use nix::fcntl::OFlag;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Call, Dest, seal};
use crate::process;
use crate::protocol::Failure;

const CAPTURE_MODE: u32 = 0o600;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Exec {
    name: String,
    #[serde(default)]
    dest: Option<Dest>,
}

pub(super) fn exec(call: &Call<'_>, args: Exec) -> Result<Value, Failure> {
    let argv = call
        .policy
        .exec(&args.name)
        .ok_or_else(|| Failure::policy(format!("{} is not an entry of exec_allow", args.name)))?;
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| Failure::policy(format!("the entry {} has no binary", args.name)))?;
    if let Some(dest) = &args.dest {
        let bytes = capture(call, dest, Path::new(program), rest, false)?;
        return Ok(json!({ "bytes": bytes }));
    }
    let output = process::succeed(Path::new(program), rest)?;
    Ok(json!({ "output": process::stdout(&output) }))
}

pub(super) fn capture(
    call: &Call<'_>,
    dest: &Dest,
    program: &Path,
    args: &[String],
    sealed: bool,
) -> Result<u64, Failure> {
    capture_with(call, dest, sealed, |file| {
        process::into_file(program, args, file).map(drop)
    })
}

pub(super) fn capture_with(
    call: &Call<'_>,
    dest: &Dest,
    sealed: bool,
    run: impl FnOnce(File) -> Result<(), Failure>,
) -> Result<u64, Failure> {
    let target = call.resolve(dest)?;
    let temporary = target.with_file_name(format!(".{}.tmp", dest.name));
    if let Err(error) = fs::remove_file(&temporary)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(Failure::io(temporary.display(), &error));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(CAPTURE_MODE)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(&temporary)
        .map_err(|error| Failure::io(temporary.display(), &error))?;
    let written = file
        .try_clone()
        .map_err(|error| Failure::io(temporary.display(), &error))
        .and_then(|mut kept| {
            run(file)?;
            if sealed {
                seal::record(&target, &mut kept)?;
            }
            fchown(&kept, Some(call.admitted), None)
                .map_err(|error| Failure::io(temporary.display(), &error))
        })
        .and_then(|()| {
            fs::rename(&temporary, &target).map_err(|error| Failure::io(target.display(), &error))
        });
    if let Err(failure) = written {
        let _ = fs::remove_file(&temporary);
        if sealed {
            let _ = seal::discard(&target);
        }
        return Err(failure);
    }
    Ok(fs::symlink_metadata(&target)
        .map_err(|error| Failure::io(target.display(), &error))?
        .len())
}
