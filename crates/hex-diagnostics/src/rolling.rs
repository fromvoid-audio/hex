//! Logger-owned rolling files. The active path stays stable; .1.log is newest
//! history, .3.log oldest. Rotation never follows or replaces symlinks, and a
//! staging file/header is prepared before moving the active file. A filesystem
//! failure can leave partial rotation; preserve evidence and report the failure.
use crate::{
    LogWriter, MAX_EVENT_BYTES, MAX_SEGMENT_BYTES, RETAINED_SEGMENTS, RETAINED_SESSIONS, Shared,
    event, unix_nanos,
};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

const NAME_ATTEMPTS: usize = 64;
const MAX_HEADER_BYTES: usize = MAX_EVENT_BYTES * 6 + 1024;
const MIN_SEGMENT_BYTES: u64 = (MAX_EVENT_BYTES * 12 + 4096) as u64;

pub(super) struct SessionWriter {
    path: PathBuf,
    writer: BufWriter<File>,
    bytes: u64,
    segment: u64,
    limit: u64,
    context: event::Context,
    started_unix: u128,
    #[cfg(test)]
    fail_staged_publish: bool,
}
impl SessionWriter {
    pub fn create(directory: &Path, context: event::Context, shared: &Shared) -> io::Result<Self> {
        Self::with_limit(directory, context, shared, MAX_SEGMENT_BYTES)
    }
    fn with_limit(
        directory: &Path,
        context: event::Context,
        shared: &Shared,
        limit: u64,
    ) -> io::Result<Self> {
        if limit < MIN_SEGMENT_BYTES {
            return Err(io::Error::other("session log segment limit is too small"));
        }
        fs::create_dir_all(directory)?;
        let (path, file) = reserve_session(directory, unix_nanos())?;
        let mut value = Self {
            path,
            writer: BufWriter::new(file),
            bytes: 0,
            segment: 0,
            limit,
            context,
            started_unix: shared.started_unix,
            #[cfg(test)]
            fail_staged_publish: false,
        };
        let header = value.header();
        if header.len() > MAX_HEADER_BYTES {
            return Err(io::Error::other("diagnostic session header exceeds budget"));
        }
        value.writer.write_all(header.as_bytes())?;
        value.writer.flush()?;
        value.bytes = header.len() as u64;
        Ok(value)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    fn archive(&self, index: usize) -> PathBuf {
        self.path.with_extension(format!("{index}.log"))
    }
    fn header(&self) -> String {
        let mut line = format!(
            "session_started log_schema=3 unix_nanos={} session={} pid={} hex_version={} segment={} max_segment_bytes={} retained_segments={} context_original_bytes={} context_truncated={} context=",
            self.started_unix,
            self.path.file_stem().unwrap_or_default().to_string_lossy(),
            std::process::id(),
            env!("CARGO_PKG_VERSION"),
            self.segment,
            self.limit,
            RETAINED_SEGMENTS,
            self.context.original_bytes,
            self.context.original_bytes > self.context.text.len(),
        );
        event::quoted(&mut line, &self.context.text);
        line.push('\n');
        line
    }
    fn rotate(&mut self) -> io::Result<()> {
        // Validate every destination before modifying history. These names are
        // reserved only for this exact newly-created session, not arbitrary files.
        for index in 1..RETAINED_SEGMENTS {
            regular_or_absent(&self.archive(index))?;
        }
        regular_or_absent(&self.path)?;
        self.writer.flush()?;
        let next_segment = self
            .segment
            .checked_add(1)
            .ok_or_else(|| io::Error::other("session log segment sequence exhausted"))?;
        let staging = self.path.with_extension("next");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        let mut next_writer = BufWriter::new(file);
        self.segment = next_segment;
        let header = self.header();
        let result = (|| -> io::Result<()> {
            next_writer.write_all(header.as_bytes())?;
            next_writer.flush()?;
            let oldest = self.archive(RETAINED_SEGMENTS - 1);
            if regular_or_absent(&oldest)? {
                fs::remove_file(&oldest)?;
            }
            for index in (1..RETAINED_SEGMENTS - 1).rev() {
                let source = self.archive(index);
                if regular_or_absent(&source)? {
                    fs::rename(source, self.archive(index + 1))?;
                }
            }
            fs::rename(&self.path, self.archive(1))?;
            #[cfg(test)]
            let publication = if self.fail_staged_publish {
                Err(io::Error::other("injected staged publication failure"))
            } else {
                fs::rename(&staging, &self.path)
            };
            #[cfg(not(test))]
            let publication = fs::rename(&staging, &self.path);
            publication.map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "active session log moved to its .1.log sibling; replacement publication failed: {error}"
                    ),
                )
            })?;
            Ok(())
        })();
        if let Err(error) = result {
            // The open prior writer/history remain evidence. Never keep growing
            // the file after a failed rollover or overwrite an unknown target.
            drop(next_writer);
            if let Err(cleanup) = fs::remove_file(&staging) {
                return Err(io::Error::other(format!(
                    "{error}; diagnostic staging cleanup failed: {cleanup}"
                )));
            }
            return Err(error);
        }
        self.writer = next_writer;
        self.bytes = header.len() as u64;
        Ok(())
    }
}
impl LogWriter for SessionWriter {
    fn line(&mut self, value: &str) -> io::Result<()> {
        let size = value.len() as u64;
        if size > self.limit.saturating_sub(MIN_SEGMENT_BYTES / 2) {
            return Err(io::Error::other(
                "serialized diagnostic record exceeds its segment budget",
            ));
        }
        if self
            .bytes
            .checked_add(size)
            .is_none_or(|total| total > self.limit)
        {
            self.rotate()?;
        }
        if self
            .bytes
            .checked_add(size)
            .is_none_or(|total| total > self.limit)
        {
            return Err(io::Error::other(
                "diagnostic segment header and record exceed budget",
            ));
        }
        self.writer.write_all(value.as_bytes())?;
        self.writer.flush()?;
        self.bytes += size;
        Ok(())
    }
}
/// Reserve the entire family, including crash-left archives/staging. A missing
/// active path does not authorize reusing or deleting an older session's data.
fn reserve_session(directory: &Path, mut stamp: u128) -> io::Result<(PathBuf, File)> {
    for _ in 0..NAME_ATTEMPTS {
        let path = directory.join(format!("session-{stamp}-{}.log", std::process::id()));
        let mut occupied = false;
        for candidate in (1..RETAINED_SEGMENTS)
            .map(|index| path.with_extension(format!("{index}.log")))
            .chain(std::iter::once(path.with_extension("next")))
        {
            match fs::symlink_metadata(candidate) {
                Ok(_) => {
                    occupied = true;
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        if !occupied {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((path, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        stamp = stamp.saturating_add(1);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a unique session log family",
    ))
}

fn regular_or_absent(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(io::Error::other(
            "session log destination is not a regular file · preserved",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Strict owned-name grammar; unrelated files and symlinks are never pruned.
fn session_key(name: &OsStr) -> Option<(u128, u32)> {
    let name = name.to_str()?.strip_prefix("session-")?;
    let (name, staging) = name
        .strip_suffix(".log")
        .map(|value| (value, false))
        .or_else(|| name.strip_suffix(".next").map(|value| (value, true)))?;
    let (stamp, tail) = name.split_once('-')?;
    if staging && tail.contains('.') {
        return None;
    }
    let (pid, segment) = tail
        .split_once('.')
        .map_or((tail, None), |(pid, segment)| (pid, Some(segment)));
    if !stamp.bytes().all(|b| b.is_ascii_digit()) || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if let Some(segment) = segment {
        let parsed = segment.parse::<usize>().ok()?;
        if parsed.to_string() != segment {
            return None;
        }
        let segment = parsed;
        if segment == 0 || segment >= RETAINED_SEGMENTS {
            return None;
        }
    }
    let timestamp = stamp.parse::<u128>().ok()?;
    let process = pid.parse::<u32>().ok()?;
    if timestamp.to_string() != stamp || process.to_string() != pid {
        return None;
    }
    Some((timestamp, process))
}
pub(super) fn prune(directory: &Path, current: &Path) -> io::Result<()> {
    let current_key = current
        .file_name()
        .and_then(session_key)
        .ok_or_else(|| io::Error::other("current diagnostic session identity is invalid"))?;
    let mut groups = BTreeMap::<_, Vec<PathBuf>>::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && let Some(key) = session_key(&entry.file_name())
        {
            groups.entry(key).or_default().push(entry.path());
        }
    }
    let mut retained = 1; // Current is protected even if the wall clock moved backward.
    let mut first_error = None;
    for (key, files) in groups.into_iter().rev() {
        if key == current_key {
            continue;
        }
        if retained < RETAINED_SESSIONS {
            retained += 1;
            continue;
        }
        for path in files {
            if let Err(error) = fs::remove_file(path)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::fixture;
    #[test]
    fn failed_staged_publication_reports_missing_active_and_preserves_history() {
        let directory = fixture();
        let shared = Shared::new();
        let mut writer = SessionWriter::with_limit(
            &directory,
            event::Context::new("test context"),
            &shared,
            MIN_SEGMENT_BYTES,
        )
        .unwrap();
        let active = writer.path.clone();
        let newest = writer.archive(1);
        let older = writer.archive(2);
        fs::write(&newest, "prior archive").unwrap();
        while writer.bytes + 4096 <= writer.limit {
            writer.line(&"x".repeat(4096)).unwrap();
        }
        let prior = fs::read(&active).unwrap();
        writer.fail_staged_publish = true;
        let (sender, receiver) = std::sync::mpsc::sync_channel(2);
        for sequence in 1..=2 {
            sender
                .send(event::Record {
                    level: crate::Level::Info,
                    category: crate::Category::Uncategorised,
                    sequence,
                    unix_nanos: 0,
                    elapsed_nanos: 0,
                    original_bytes: 4096,
                    message: "x".repeat(4096),
                })
                .unwrap();
        }
        shared
            .closing
            .store(true, std::sync::atomic::Ordering::Release);
        crate::run_records(&mut writer, &receiver, &shared);

        assert!(!active.exists());
        assert_eq!(fs::read(&newest).unwrap(), prior);
        assert_eq!(fs::read_to_string(&older).unwrap(), "prior archive");
        assert!(!active.with_extension("next").exists());
        assert_eq!(shared.statistics().write_failed, 1);
        assert_eq!(shared.statistics().abandoned, 1);
        let failure = shared.last_error.lock().unwrap().clone().unwrap();
        assert!(failure.contains("injected staged publication failure"));
        assert!(
            failure.contains("active session log moved to its .1.log sibling"),
            "{failure}"
        );
        drop(writer);
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn rolls_bounded_recent_segments_with_context_and_keeps_active_path() {
        let directory = fixture();
        let shared = Shared::new();
        let context = format!(
            "build=original flags=actions {}",
            "€".repeat(MAX_EVENT_BYTES)
        );
        let mut writer = SessionWriter::with_limit(
            &directory,
            event::Context::new(&context),
            &shared,
            MIN_SEGMENT_BYTES,
        )
        .unwrap();
        let path = writer.path.clone();
        for sequence in 1..=160 {
            let line = format!("event sequence={sequence} {}\n", "x".repeat(4096));
            writer.line(&line).unwrap();
        }
        assert_eq!(writer.path(), path);
        let files: Vec<_> = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), RETAINED_SEGMENTS);
        for file in &files {
            assert!(fs::metadata(file).unwrap().len() <= MIN_SEGMENT_BYTES);
            let contents = fs::read_to_string(file).unwrap();
            assert!(contents.starts_with("session_started log_schema=3"));
            assert!(contents.contains("build=original flags=actions"));
            assert!(contents.contains("context_truncated=true"));
            assert!(contents.contains(&format!("context_original_bytes={}", context.len())));
        }
        assert!(
            fs::read_to_string(path)
                .unwrap()
                .contains("event sequence=160")
        );
        drop(writer);
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    #[cfg(unix)]
    fn rotation_refuses_symlink_and_preserves_its_target() {
        let directory = fixture();
        let shared = Shared::new();
        let mut writer = SessionWriter::with_limit(
            &directory,
            event::Context::new(""),
            &shared,
            MIN_SEGMENT_BYTES,
        )
        .unwrap();
        let unrelated = directory.join("preserve.txt");
        fs::write(&unrelated, "original").unwrap();
        let link = writer.archive(1);
        std::os::unix::fs::symlink(&unrelated, &link).unwrap();
        while writer.bytes + 4096 <= writer.limit {
            writer.line(&"x".repeat(4096)).unwrap();
        }
        assert!(writer.line(&"x".repeat(4096)).is_err());
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "original");
        assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
        drop(writer);
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn retention_groups_segments_and_protects_current_when_clock_moves_backward() {
        let directory = fixture();
        for stamp in 1..=12 {
            for segment in ["", ".1", ".2"] {
                fs::write(
                    directory.join(format!("session-{stamp}-7{segment}.log")),
                    "old",
                )
                .unwrap();
            }
        }
        fs::write(directory.join("session-2-7.next"), "orphan header").unwrap();
        let current = directory.join("session-1-7.log");
        let unrelated = directory.join("session-0-7.999.log");
        fs::write(&unrelated, "preserve").unwrap();
        prune(&directory, &current).unwrap();
        assert!(current.exists());
        assert!(unrelated.exists());
        let mut keys = std::collections::BTreeSet::new();
        for entry in fs::read_dir(&directory).unwrap() {
            if let Some(key) = session_key(&entry.unwrap().file_name()) {
                keys.insert(key);
            }
        }
        assert_eq!(keys.len(), RETAINED_SESSIONS);
        assert!(!directory.join("session-2-7.log").exists());
        assert!(!directory.join("session-2-7.next").exists());
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn missing_active_file_does_not_allow_reusing_a_crashed_session_family() {
        let directory = fixture();
        let root = directory.join(format!("session-5-{}.log", std::process::id()));
        let old = root.with_extension("1.log");
        fs::write(&old, "preserve original").unwrap();
        let (path, file) = reserve_session(&directory, 5).unwrap();
        assert_ne!(path, root);
        assert_eq!(fs::read_to_string(old).unwrap(), "preserve original");
        drop(file);
        fs::remove_dir_all(directory).unwrap();
    }
}
