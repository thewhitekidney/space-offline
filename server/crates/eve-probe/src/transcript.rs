//! What the probe leaves behind.
//!
//! Three files per run, because they answer three different questions:
//!
//! | file | question |
//! |---|---|
//! | `<stem>.log` | *what happened* — a timestamped narrative a human reads |
//! | `<stem>.evecap` | *what the bytes were* — replayable, and readable by `evecap` |
//! | `<stem>.schemas.md` | *what shapes came back* — the artefact of the run |
//!
//! The `.evecap` is deliberately the **same format `eve-server` writes**
//! (`eve_server::capture`, documented there): a 14-byte magic, a version
//! byte, then `dir | micros | len | bytes` per frame. Not because the format
//! is good, but because the project already has a reader for it, so a probe
//! transcript can be opened with `evecap --calls` the moment it exists. The
//! writer is twenty lines; a second format would have cost a tool.
//!
//! Direction is from the **probe's** point of view, which is the client's:
//! [`Dir::Out`] is recorded as client->server.
//!
//! Nothing here interprets a payload. The log is written by the caller, so
//! what the transcript says about a frame is the caller's reading of it, and
//! the `.evecap` beside it is the bytes to check that reading against.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Mirrors `eve_server::capture::MAGIC`, so the two formats are one format.
pub const CAP_MAGIC: &[u8; 14] = b"EVEOFFLINECAP\n";
pub const CAP_VERSION: u8 = 1;

/// Which way a frame travelled, from the probe's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Probe -> server. Recorded as the capture's client->server.
    Out,
    /// Server -> probe.
    In,
}

impl Dir {
    fn code(self) -> u8 {
        match self {
            Dir::Out => 0,
            Dir::In => 1,
        }
    }

    fn arrow(self) -> &'static str {
        match self {
            Dir::Out => ">>",
            Dir::In => "<<",
        }
    }
}

/// The three output files of one run.
pub struct Transcript {
    cap: BufWriter<File>,
    log: BufWriter<File>,
    schemas: BufWriter<File>,
    started: Instant,
    paths: [PathBuf; 3],
}

impl Transcript {
    /// Create `<dir>/<stem>.{log,evecap,schemas.md}`.
    pub fn create(dir: &Path, stem: &str) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let paths = [
            dir.join(format!("{stem}.log")),
            dir.join(format!("{stem}.evecap")),
            dir.join(format!("{stem}.schemas.md")),
        ];
        let mut cap = BufWriter::new(File::create(&paths[1])?);
        cap.write_all(CAP_MAGIC)?;
        cap.write_all(&[CAP_VERSION])?;
        Ok(Transcript {
            cap,
            log: BufWriter::new(File::create(&paths[0])?),
            schemas: BufWriter::new(File::create(&paths[2])?),
            started: Instant::now(),
            paths,
        })
    }

    /// `[log, evecap, schemas]`, for telling the reader where they are.
    pub fn paths(&self) -> &[PathBuf; 3] {
        &self.paths
    }

    pub fn millis(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    /// A line in the narrative, stamped.
    pub fn line(&mut self, text: &str) -> io::Result<()> {
        writeln!(self.log, "[{:9.1}ms] {text}", self.millis())
    }

    /// A continuation line, indented under the previous one and unstamped.
    pub fn detail(&mut self, text: &str) -> io::Result<()> {
        for l in text.lines() {
            writeln!(self.log, "              {l}")?;
        }
        Ok(())
    }

    /// A section break. The log is meant to be skimmed, and the stages are
    /// the thing a reader is looking for.
    pub fn heading(&mut self, text: &str) -> io::Result<()> {
        writeln!(self.log, "\n=== {text} ===")
    }

    /// Record one frame's bytes and note it in the narrative.
    pub fn frame(&mut self, dir: Dir, bytes: &[u8], note: &str) -> io::Result<()> {
        let micros = self.started.elapsed().as_micros() as u64;
        self.cap.write_all(&[dir.code()])?;
        self.cap.write_all(&micros.to_le_bytes())?;
        self.cap.write_all(&(bytes.len() as u32).to_le_bytes())?;
        self.cap.write_all(bytes)?;
        writeln!(
            self.log,
            "[{:9.1}ms] {} {:>8} bytes  {note}",
            self.millis(),
            dir.arrow(),
            bytes.len()
        )
    }

    /// A line in the schema summary. This file is the deliverable, so it is
    /// written as markdown and nothing else goes in it.
    pub fn schema(&mut self, text: &str) -> io::Result<()> {
        writeln!(self.schemas, "{text}")
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.cap.flush()?;
        self.log.flush()?;
        self.schemas.flush()
    }
}

impl Drop for Transcript {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

/// Read back an `.evecap`: `(direction, micros, bytes)` per frame.
///
/// The probe's own replay path, and the reason the format is shared: a
/// recorded session can be decoded again without a server.
pub fn read_capture(bytes: &[u8]) -> Result<Vec<(Dir, u64, Vec<u8>)>, String> {
    if bytes.len() < 15 || &bytes[..14] != CAP_MAGIC {
        return Err("not an evecap file".into());
    }
    if bytes[14] != CAP_VERSION {
        return Err(format!("unsupported capture version {}", bytes[14]));
    }
    let mut out = Vec::new();
    let mut at = 15;
    while at < bytes.len() {
        if at + 13 > bytes.len() {
            return Err(format!("truncated record header at {at}"));
        }
        let dir = match bytes[at] {
            0 => Dir::Out,
            1 => Dir::In,
            other => return Err(format!("unknown direction {other} at {at}")),
        };
        let micros = u64::from_le_bytes(bytes[at + 1..at + 9].try_into().expect("8 bytes"));
        let len = u32::from_le_bytes(bytes[at + 9..at + 13].try_into().expect("4 bytes")) as usize;
        at += 13;
        if at + len > bytes.len() {
            return Err(format!("truncated record body at {at}"));
        }
        out.push((dir, micros, bytes[at..at + len].to_vec()));
        at += len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The format is shared with `eve-server`, so pin the two constants that
    /// make it shared. A silent change here breaks `evecap` on our files
    /// with no compile error anywhere.
    #[test]
    fn the_capture_header_is_eve_servers() {
        assert_eq!(CAP_MAGIC, b"EVEOFFLINECAP\n");
        assert_eq!(CAP_MAGIC.len(), 14);
        assert_eq!(CAP_VERSION, 1);
    }

    #[test]
    fn capture_records_round_trip() {
        let mut file = Vec::new();
        file.extend_from_slice(CAP_MAGIC);
        file.push(CAP_VERSION);
        for (dir, micros, body) in [(0u8, 7u64, b"ab".as_slice()), (1, 99, b"")] {
            file.push(dir);
            file.extend_from_slice(&micros.to_le_bytes());
            file.extend_from_slice(&(body.len() as u32).to_le_bytes());
            file.extend_from_slice(body);
        }
        let records = read_capture(&file).expect("valid capture");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], (Dir::Out, 7, b"ab".to_vec()));
        assert_eq!(records[1], (Dir::In, 99, Vec::new()));
    }

    #[test]
    fn a_truncated_capture_is_an_error_not_a_panic() {
        let mut file = Vec::new();
        file.extend_from_slice(CAP_MAGIC);
        file.push(CAP_VERSION);
        file.push(0);
        file.extend_from_slice(&7u64.to_le_bytes());
        file.extend_from_slice(&64u32.to_le_bytes());
        file.extend_from_slice(b"short");
        assert!(read_capture(&file).is_err());
        assert!(read_capture(b"nope").is_err());
        assert!(read_capture(&[]).is_err());
    }
}
