//! Address publication for the local HTTP server.
//!
//! `fxg node start` and `fxg hub start` publish the bound address in
//! `<state>/<role>/<role>.addr`, and the CLI treats the file as the liveness
//! signal for the local process. `fxg node restart` / `fxg hub restart` start
//! the replacement process before the old one finishes shutting down, and the
//! replacement may bind the same address. Comparing the file contents therefore
//! cannot tell the old process whether the file still belongs to it: a
//! shutting-down daemon would delete the replacement's file when both bound the
//! same `127.0.0.1:7860` / `127.0.0.1:8080`.
//!
//! Each publish records a unique generation in a sidecar lock file
//! (`<role>.addr.lock`) while holding an exclusive advisory lock, and writes the
//! address file with a temp file + rename. A daemon withdraws the address only
//! while the recorded generation is still its own, so a replacement publisher is
//! never overwritten or removed.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Address file published by a running daemon.
///
/// Created by [`AddressFile::publish`] and removed by
/// [`AddressFile::withdraw`] during graceful shutdown.
#[derive(Debug)]
pub struct AddressFile {
    path: PathBuf,
    generation: String,
}

impl AddressFile {
    /// Publishes `addr` to `path` and records this process as the owner.
    ///
    /// The generation and the address are written under an exclusive lock, so a
    /// concurrent replacement publisher is fully ordered either before or after
    /// this call.
    pub fn publish(path: &Path, addr: &str) -> io::Result<Self> {
        let generation = new_generation();
        let mut lock = lock_file(path)?;
        lock.set_len(0)?;
        lock.seek(SeekFrom::Start(0))?;
        lock.write_all(generation.as_bytes())?;
        lock.flush()?;
        write_atomically(path, addr)?;
        drop(lock);
        Ok(Self {
            path: path.to_path_buf(),
            generation,
        })
    }

    /// Removes the address file when it still belongs to this process.
    ///
    /// Returns `false` when a replacement daemon has published its own
    /// generation in the meantime; the replacement's file is left untouched.
    pub fn withdraw(&self) -> io::Result<bool> {
        let mut lock = lock_file(&self.path)?;
        lock.seek(SeekFrom::Start(0))?;
        let mut current = String::new();
        lock.read_to_string(&mut current)?;
        if current.trim() != self.generation {
            return Ok(false);
        }
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        lock.set_len(0)?;
        Ok(true)
    }

    /// Path of the published address file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Returns a path next to `path` with `suffix` appended to the file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Opens the sidecar lock file and takes an exclusive lock.
fn lock_file(path: &Path) -> io::Result<File> {
    let lock_path = with_suffix(path, ".lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        // The generation written by the previous owner is read here; truncation
        // happens under the lock when a new generation is published.
        .truncate(false)
        .open(&lock_path)?;
    file.lock()?;
    Ok(file)
}

/// Writes `contents` with a temp file + rename so readers never see a partial file.
fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    let tmp = with_suffix(path, &format!(".{}.tmp", std::process::id()));
    let result = std::fs::write(&tmp, contents).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        fxg_protocol::discard_with!(
            std::fs::remove_file(&tmp),
            "remove a temporary address file"
        );
    }
    result
}

/// Unique value identifying one publisher (process id + random suffix).
fn new_generation() -> String {
    format!("{}-{:016x}", std::process::id(), rand::random::<u64>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_then_withdraw_removes_the_address() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("node.addr");
        let addr = AddressFile::publish(&path, "127.0.0.1:7860").expect("publish");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read addr"),
            "127.0.0.1:7860"
        );
        assert!(addr.withdraw().expect("withdraw"));
        assert!(!path.exists());
    }

    #[test]
    fn replacement_keeps_its_address_when_the_old_publisher_withdraws() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("node.addr");
        let old = AddressFile::publish(&path, "127.0.0.1:7860").expect("publish old");
        let new = AddressFile::publish(&path, "127.0.0.1:7860").expect("publish new");

        assert!(!old.withdraw().expect("old withdraw"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read addr"),
            "127.0.0.1:7860"
        );

        assert!(new.withdraw().expect("new withdraw"));
        assert!(!path.exists());
    }

    #[test]
    fn withdraw_twice_does_not_remove_a_later_publisher() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hub.addr");
        let first = AddressFile::publish(&path, "127.0.0.1:8080").expect("publish first");
        assert!(first.withdraw().expect("withdraw first"));
        let second = AddressFile::publish(&path, "127.0.0.1:8080").expect("publish second");
        assert!(!first.withdraw().expect("withdraw first again"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read addr"),
            "127.0.0.1:8080"
        );
        assert!(second.withdraw().expect("withdraw second"));
    }
}
