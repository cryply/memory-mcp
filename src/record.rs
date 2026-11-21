use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::event::Event;

// The record: JSONL log split by month plus content-addressed source blobs.
// This is the only authoritative data. Everything else is rebuilt from it.
// Several server processes may share one record; writes are serialized by
// an exclusive flock on record/.lock.
pub struct Record {
    root: PathBuf,
}

pub struct Lock {
    _file: fs::File,
}

impl Record {
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root.join("log"))?;
        fs::create_dir_all(root.join("sources"))?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }


    // Blocks until the lock is ours. Released on drop.
    pub fn lock(&self) -> Result<Lock> {
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(self.root.join(".lock"))?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error()).context("flock");
        }
        Ok(Lock { _file: file })
    }

    // Caller must hold the lock.
    pub fn append(&self, _lock: &Lock, ev: &Event) -> Result<()> {
        let month = &ev.tx[..7];
        let path = self.root.join("log").join(format!("{}.jsonl", month));
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        let mut line = serde_json::to_string(ev)?;
        line.push('\n');
        f.write_all(line.as_bytes())?;
        f.sync_data()?;
        Ok(())
    }

    pub fn put_source(&self, bytes: &[u8]) -> Result<String> {
        let hash = hex::encode(Sha256::digest(bytes));
        let path = self.root.join("sources").join(&hash);
        if !path.exists() {
            let tmp = path.with_extension(format!("tmp{}", std::process::id()));
            fs::write(&tmp, bytes)?;
            fs::rename(tmp, path)?;
        }
        Ok(hash)
    }

    pub fn get_source(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        let path = self.root.join("sources").join(hash);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(fs::read(path)?))
    }

    pub fn log_files(&self) -> Result<Vec<PathBuf>> {
        let mut files: Vec<PathBuf> = fs::read_dir(self.root.join("log"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false))
            .collect();
        files.sort();
        Ok(files)
    }

    // Reads complete events from `path` starting at byte `offset`.
    // Returns the events and the offset just past the last complete line.
    pub fn read_from(&self, path: &Path, offset: u64) -> Result<(Vec<Event>, u64)> {
        let mut f = fs::File::open(path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(f);
        let mut pos = offset;
        let mut out = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 || !line.ends_with('\n') {
                break;
            }
            pos += n as u64;
            if line.trim().is_empty() {
                continue;
            }
            let ev: Event = serde_json::from_str(&line)
                .with_context(|| format!("{} @{}", path.display(), pos))?;
            out.push(ev);
        }
        Ok((out, pos))
    }

    // Commits the record if it is a git repository. Caller must hold the lock.
    // Best effort: a failure is reported on stderr and does not fail the write.
    pub fn git_commit(&self, _lock: &Lock, message: &str) {
        if !self.root.join(".git").exists() {
            return;
        }
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(args)
                .output()
        };
        match run(&["add", "-A"]) {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                eprintln!("git add: {}", String::from_utf8_lossy(&o.stderr).trim());
                return;
            }
            Err(e) => {
                eprintln!("git add: {}", e);
                return;
            }
        }
        match run(&["commit", "-q", "-m", message]) {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                if !err.contains("nothing to commit") {
                    eprintln!("git commit: {}", err.trim());
                }
            }
            Err(e) => eprintln!("git commit: {}", e),
        }
    }
}
