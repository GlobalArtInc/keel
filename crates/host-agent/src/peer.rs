use std::io;

use tokio::net::UnixStream;

#[cfg(target_os = "linux")]
pub(crate) fn uid(stream: &UnixStream) -> io::Result<u32> {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};

    getsockopt(stream, PeerCredentials)
        .map(|credentials| credentials.uid())
        .map_err(io::Error::from)
}

#[cfg(target_os = "macos")]
pub(crate) fn uid(stream: &UnixStream) -> io::Result<u32> {
    nix::unistd::getpeereid(stream)
        .map(|(uid, _)| uid.as_raw())
        .map_err(io::Error::from)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!(
    "the agent reads peer credentials with SO_PEERCRED on Linux or getpeereid on macOS; no other target is supported"
);
