//! One snapframe at a time: the running one listens on a socket in
//! `$XDG_RUNTIME_DIR`; starting another sends it "toggle" instead (cancel the
//! overlay, or stop the recording), which is what pressing the bind again means.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

pub const TOGGLE: &str = "toggle";

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
    dir.join("snapframe.sock")
}

/// The listener a running instance owns; removes its socket when dropped.
pub struct Instance {
    pub listener: UnixListener,
    path: PathBuf,
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub enum Claim {
    /// We are the only instance now.
    Owned(Instance),
    /// Another instance is running and was sent the message.
    Forwarded,
}

/// Becomes the running instance, or forwards `message` to the one that is.
pub fn claim(message: &str) -> Result<Claim> {
    let path = socket_path();
    if send(&path, message).is_ok() {
        return Ok(Claim::Forwarded);
    }
    // Nobody answered: any socket file left is stale.
    let _ = std::fs::remove_file(&path);
    let listener =
        UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    listener.set_nonblocking(true)?;
    Ok(Claim::Owned(Instance { listener, path }))
}

/// Sends `message` to the running instance and returns its reply.
pub fn send(path: &std::path::Path, message: &str) -> Result<String> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    writeln!(stream, "{message}")?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    Ok(reply.trim().to_owned())
}

impl Instance {
    /// Accepts pending connections and returns their messages, replying "ok".
    pub fn drain(&self) -> Vec<String> {
        let mut messages = Vec::new();
        while let Ok((stream, _)) = self.listener.accept() {
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut line = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut line).is_ok() {
                let _ = (&stream).write_all(b"ok\n");
                messages.push(line.trim().to_owned());
            }
        }
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_claim_is_forwarded_to_the_first() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests in this module don't read the variable concurrently.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", dir.path()) };
        let Claim::Owned(first) = claim(TOGGLE).unwrap() else {
            panic!("expected to own")
        };
        let server = std::thread::spawn(move || {
            loop {
                let got = first.drain();
                if !got.is_empty() {
                    return got;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        assert!(matches!(claim(TOGGLE).unwrap(), Claim::Forwarded));
        assert_eq!(server.join().unwrap(), vec![TOGGLE.to_owned()]);
        // The first instance dropped its socket, so the next claim owns.
        assert!(matches!(claim(TOGGLE).unwrap(), Claim::Owned(_)));
    }
}
