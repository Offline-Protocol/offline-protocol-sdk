//! The file mechanics both built-in stores share: the record frame, atomic
//! durable writes, bounded reads, bounded enumeration and the stale-temporary
//! sweep.
//!
//! Nothing here knows about sealing or about either trait's error type.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use rand_core::{OsRng, RngCore};

use crate::protocol_state_storage::MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES;

/// Magic of a framed record, shared with every binding's state provider.
pub(super) const RECORD_MAGIC: &[u8; 4] = b"OPS1";

/// Longest accepted `key_type` or `key_id`, in UTF-8 bytes. Mirrors the
/// bindings' `MAX_COMPONENT_BYTES`.
pub(super) const MAX_COMPONENT_BYTES: usize = 4096;

/// Largest value either store accepts. The protocol-state ceiling, which the
/// bindings mirror; for MLS material it is far above the largest record the
/// engine writes (an OpenMLS ratchet tree, about 1.1 KiB per member).
pub(super) const MAX_VALUE_BYTES: usize = MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES;

/// Longest possible frame header.
pub(super) const MAX_HEADER_BYTES: usize = 8 + 2 * MAX_COMPONENT_BYTES;

/// Largest frame, and so the largest plain record file.
pub(super) const MAX_FRAME_BYTES: usize = MAX_HEADER_BYTES + MAX_VALUE_BYTES;

/// Ceiling on entries one enumeration examines. Counts entries opened, not
/// keys returned: an entry that does not parse yields no key, and a directory
/// of them must not be walked in full on every launch.
pub(super) const MAX_LISTED_ENTRIES: usize = 65_536;

/// Prefix of the temporary a write renames into place. Distinct from every
/// entry prefix, so enumeration never mistakes a half-written record for one.
pub(super) const TEMP_PREFIX: &str = ".write-";

/// Ceiling on entries one stale-temporary sweep examines.
const MAX_SWEEP_ENTRIES: usize = 4_096;

/// Age past which a temporary is presumed orphaned by a crash.
///
/// The bindings sweep every temporary unconditionally and rely on one lock
/// ordering every writer in the process. That holds for one process; a second
/// process over the same directory (unsupported, but a misconfigured service
/// manager can start one) would have its in-flight write unlinked and fail a
/// store it should not have. No live write takes this long.
const STALE_TEMPORARY_AGE: Duration = Duration::from_secs(300);

/// Owner-only mode for every directory a store creates.
#[cfg(unix)]
const DIRECTORY_MODE: u32 = 0o700;

/// Owner-only mode for every record file.
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;

/// Serialises every store operation in the process.
///
/// Process-wide rather than per store, for the reason the bindings give: two
/// stores over one root are not hypothetical, and a per-instance lock cannot
/// order them. The operations are short file calls, and the engine already
/// serialises storage behind its own lock.
static STORE_LOCK: Mutex<()> = Mutex::new(());

/// Takes the process-wide store lock. A panic while it was held cannot have
/// left a record torn (every write is a rename), so a poisoned lock is still
/// safe to take.
pub(super) fn lock() -> MutexGuard<'static, ()> {
    STORE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Why a frame could not be built.
#[derive(Debug)]
pub(super) enum FrameError {
    ComponentTooLong,
    ValueTooLarge(usize),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ComponentTooLong => {
                write!(f, "key type or key id exceeds {MAX_COMPONENT_BYTES} bytes")
            }
            Self::ValueTooLarge(len) => write!(
                f,
                "record is {len} bytes, over the {MAX_VALUE_BYTES} byte limit"
            ),
        }
    }
}

/// Frames a record:
///
/// ```text
/// bytes 0..4   magic "OPS1"
/// bytes 4..6   key_type length, big-endian u16
/// bytes 6..8   key_id length, big-endian u16
/// then         key_type UTF-8, key_id UTF-8, value bytes
/// ```
///
/// The bindings' state providers write exactly these bytes.
pub(super) fn frame(key_type: &str, key_id: &str, value: &[u8]) -> Result<Vec<u8>, FrameError> {
    let (type_len, id_len) = match (u16::try_from(key_type.len()), u16::try_from(key_id.len())) {
        (Ok(t), Ok(i))
            if usize::from(t) <= MAX_COMPONENT_BYTES && usize::from(i) <= MAX_COMPONENT_BYTES =>
        {
            (t, i)
        }
        _ => return Err(FrameError::ComponentTooLong),
    };
    if value.len() > MAX_VALUE_BYTES {
        return Err(FrameError::ValueTooLarge(value.len()));
    }
    let mut out = Vec::with_capacity(8 + key_type.len() + key_id.len() + value.len());
    out.extend_from_slice(RECORD_MAGIC);
    out.extend_from_slice(&type_len.to_be_bytes());
    out.extend_from_slice(&id_len.to_be_bytes());
    out.extend_from_slice(key_type.as_bytes());
    out.extend_from_slice(key_id.as_bytes());
    out.extend_from_slice(value);
    Ok(out)
}

/// A parsed frame header.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Header {
    pub(super) key_type: String,
    pub(super) key_id: String,
    pub(super) value_offset: usize,
}

/// Parses a frame header, or `None` when `raw` is not a frame this SDK wrote.
pub(super) fn parse_header(raw: &[u8]) -> Option<Header> {
    if raw.len() < 8 || &raw[..4] != RECORD_MAGIC {
        return None;
    }
    let type_len = usize::from(u16::from_be_bytes([raw[4], raw[5]]));
    let id_len = usize::from(u16::from_be_bytes([raw[6], raw[7]]));
    if type_len > MAX_COMPONENT_BYTES || id_len > MAX_COMPONENT_BYTES {
        return None;
    }
    let id_start = 8 + type_len;
    let value_offset = id_start + id_len;
    if raw.len() < value_offset {
        return None;
    }
    let key_type = std::str::from_utf8(&raw[8..id_start]).ok()?.to_string();
    let key_id = std::str::from_utf8(&raw[id_start..value_offset])
        .ok()?
        .to_string();
    Some(Header {
        key_type,
        key_id,
        value_offset,
    })
}

/// Creates `directory` owner-only, tightening it if it already exists.
///
/// The mode set at creation is masked by the umask and does nothing for a
/// directory that is already there, so the explicit `set_permissions` is what
/// makes it deterministic. Best effort on that step: a filesystem without
/// POSIX permissions refuses it, and there the mode was never a control.
pub(super) fn private_mkdir(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(DIRECTORY_MODE)
            .create(directory)?;
        let _ = fs::set_permissions(directory, fs::Permissions::from_mode(DIRECTORY_MODE));
    }
    #[cfg(not(unix))]
    fs::create_dir_all(directory)?;
    Ok(())
}

/// Flushes a directory entry so a rename or unlink in it survives a crash.
///
/// Best effort: a directory cannot be opened for `fsync` on every platform
/// (notably Windows), and there the file-level flush is the strongest
/// guarantee available.
pub(super) fn sync_directory(directory: &Path) {
    if let Ok(handle) = File::open(directory) {
        let _ = handle.sync_all();
    }
}

/// Writes `bytes` to `target` atomically and durably: a fresh owner-only
/// temporary in the same directory, flushed, renamed over the target, and
/// the directory flushed. A reader sees the old record or the new one, never
/// a torn one, and an acknowledged write survives power loss.
pub(super) fn write_atomic(directory: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let (temporary, mut handle) = create_temporary(directory)?;
    let result = (|| {
        handle.write_all(bytes)?;
        handle.sync_all()?;
        drop(handle);
        fs::rename(&temporary, target)?;
        sync_directory(directory);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_temporary(directory: &Path) -> io::Result<(PathBuf, File)> {
    // `create_new` makes a name collision an error rather than a shared file,
    // so a retry on a fresh name is all a collision costs.
    for _ in 0..8 {
        let mut suffix = [0u8; 12];
        OsRng.fill_bytes(&mut suffix);
        let path = directory.join(format!("{TEMP_PREFIX}{}", hex::encode(suffix)));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(FILE_MODE);
        }
        match options.open(&path) {
            Ok(handle) => return Ok((path, handle)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a unique temporary file",
    ))
}

/// The result of a bounded read.
pub(super) enum BoundedRead {
    Absent,
    /// Over the ceiling; nothing past the `stat` (or the ceiling) was read.
    Oversized,
    Bytes(Vec<u8>),
}

/// Reads `path` whole, refusing anything over `max` without reading it.
///
/// The size is checked before the read, and the read itself stops one byte
/// past the ceiling so a size that raced a writer (or a filesystem that
/// misreports it) is still caught.
pub(super) fn read_bounded(path: &Path, max: usize) -> io::Result<BoundedRead> {
    let handle = match File::open(path) {
        Ok(handle) => handle,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(BoundedRead::Absent),
        Err(err) => return Err(err),
    };
    let reported = handle.metadata()?.len();
    if reported > max as u64 {
        return Ok(BoundedRead::Oversized);
    }
    // Capacity from the stat, never from the ceiling: a small record must
    // not reserve eight megabytes.
    let mut raw = Vec::with_capacity(usize::try_from(reported).unwrap_or(max).min(max));
    handle.take(max as u64 + 1).read_to_end(&mut raw)?;
    if raw.len() > max {
        return Ok(BoundedRead::Oversized);
    }
    Ok(BoundedRead::Bytes(raw))
}

/// Reads at most `max` bytes from the start of `path`; `None` on any failure.
pub(super) fn read_prefix(path: &Path, max: usize) -> Option<Vec<u8>> {
    let handle = File::open(path).ok()?;
    let mut raw = Vec::new();
    handle.take(max as u64).read_to_end(&mut raw).ok()?;
    Some(raw)
}

/// Removes `path`, flushing its directory. `Ok(false)` when it was absent.
pub(super) fn remove(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => {
            if let Some(parent) = path.parent() {
                sync_directory(parent);
            }
            Ok(true)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

/// Calls `visit` for up to `limit` regular files in `directory` whose name
/// starts with `prefix`. A missing directory is empty.
pub(super) fn for_each_entry(
    directory: &Path,
    prefix: &str,
    limit: usize,
    mut visit: impl FnMut(&Path, &str),
) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let mut examined = 0usize;
    for entry in entries {
        if examined >= limit {
            break;
        }
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(prefix) {
            continue;
        }
        examined += 1;
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        visit(&entry.path(), name);
    }
    Ok(())
}

/// Remembers which directories a store has swept, so the sweep runs once per
/// directory per store and stays off the restore path.
#[derive(Default)]
pub(super) struct SweepMemo(Mutex<HashSet<PathBuf>>);

impl SweepMemo {
    /// Removes temporaries a crashed writer left in `directory`, once.
    ///
    /// A write renames a temporary into place, so a crash between the two
    /// orphans it for good: enumeration filters on the entry prefix and never
    /// sees it. Best effort throughout: a directory that cannot be scanned is
    /// no reason to fail the store about to write into it.
    pub(super) fn sweep_once(&self, directory: &Path) {
        {
            let mut swept = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !swept.insert(directory.to_path_buf()) {
                return;
            }
        }
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        let now = SystemTime::now();
        for entry in entries.take(MAX_SWEEP_ENTRIES).flatten() {
            let name = entry.file_name();
            if !name.to_str().is_some_and(|n| n.starts_with(TEMP_PREFIX)) {
                continue;
            }
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= STALE_TEMPORARY_AGE);
            if stale && fs::remove_file(entry.path()).is_ok() {
                tracing::debug!(name = ?name, "removed stale store temporary");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_store::test_support::TempRoot;

    #[test]
    fn a_header_is_refused_unless_it_names_a_whole_key() {
        let framed = frame("outbox", "m-1", b"\xaa").expect("frame");
        assert!(parse_header(&framed[..framed.len() - 2]).is_none());
        assert!(parse_header(b"OPS").is_none());
        assert!(parse_header(b"OPS2\x00\x00\x00\x00").is_none());
        // A length over the component ceiling is refused before any slice.
        assert!(parse_header(b"OPS1\xff\xff\x00\x00").is_none());
        // Invalid UTF-8 in a component is not a key.
        assert!(parse_header(b"OPS1\x00\x01\x00\x00\xff").is_none());
    }

    #[test]
    fn a_component_or_value_over_the_ceiling_is_refused() {
        let long = "x".repeat(MAX_COMPONENT_BYTES + 1);
        assert!(matches!(
            frame(&long, "id", b""),
            Err(FrameError::ComponentTooLong)
        ));
        assert!(matches!(
            frame("t", &long, b""),
            Err(FrameError::ComponentTooLong)
        ));
        assert!(frame(&"x".repeat(MAX_COMPONENT_BYTES), "id", b"").is_ok());
        let big = vec![0u8; MAX_VALUE_BYTES + 1];
        assert!(matches!(
            frame("t", "id", &big),
            Err(FrameError::ValueTooLarge(_))
        ));
    }

    #[test]
    fn a_fresh_temporary_survives_the_sweep_and_an_old_one_does_not() {
        let root = TempRoot::new("sweep");
        let fresh = root.path().join(format!("{TEMP_PREFIX}fresh"));
        let old = root.path().join(format!("{TEMP_PREFIX}old"));
        let entry = root.path().join("k_entry");
        for path in [&fresh, &old, &entry] {
            fs::write(path, b"x").expect("write");
        }
        let long_ago = SystemTime::now() - STALE_TEMPORARY_AGE - Duration::from_secs(60);
        for path in [&old, &entry] {
            File::options()
                .write(true)
                .open(path)
                .and_then(|h| h.set_modified(long_ago))
                .expect("backdate");
        }

        SweepMemo::default().sweep_once(root.path());

        assert!(fresh.exists(), "an in-flight write must not be unlinked");
        assert!(!old.exists(), "a crashed write's temporary must be removed");
        assert!(entry.exists(), "the sweep must never touch a record");
    }

    #[test]
    fn a_bounded_read_refuses_by_size_without_reading() {
        let root = TempRoot::new("bounded");
        let path = root.path().join("big");
        let handle = File::create(&path).expect("create");
        // Sparse, so this costs no real disk.
        handle.set_len(16 * 1024).expect("set_len");
        drop(handle);
        assert!(matches!(
            read_bounded(&path, 1024).expect("read"),
            BoundedRead::Oversized
        ));
        assert!(matches!(
            read_bounded(&root.path().join("absent"), 1024).expect("read"),
            BoundedRead::Absent
        ));
    }
}
