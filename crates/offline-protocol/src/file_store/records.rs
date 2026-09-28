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

/// Ceiling on temporaries one stale-temporary sweep examines.
///
/// Counts temporaries, not directory entries: a cap on entries would let a
/// directory with more records than the cap hide an orphan behind them for
/// good, because the enumeration order is the same on every launch.
const MAX_SWEEP_TEMPORARIES: usize = 4_096;

/// Ceiling on directory entries one sweep reads, so a tampered directory
/// cannot turn the first write of a session into an unbounded scan. The same
/// bound enumeration already pays.
const MAX_SWEEP_SCANNED: usize = MAX_LISTED_ENTRIES;

/// Age past which a temporary is presumed orphaned by a crash.
///
/// The bindings sweep every temporary unconditionally and rely on one lock
/// ordering every writer in the process. The built-in stores refuse a second
/// store over their directory ([`DirectoryLock`]), but on a filesystem that
/// cannot lock, a second process would have its in-flight write unlinked and
/// fail a store it should not have. The age keeps that case safe too. No live
/// write takes this long.
const STALE_TEMPORARY_AGE: Duration = Duration::from_secs(300);

/// Owner-only mode for every directory a store creates.
#[cfg(unix)]
const DIRECTORY_MODE: u32 = 0o700;

/// Owner-only mode for every record file.
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;

/// Serialises one store's operations.
///
/// Per store, not per process. The bindings use one process-wide lock because
/// two of their providers over one directory are possible and an instance
/// lock cannot order them. Here that case cannot arise: each store holds a
/// [`DirectoryLock`] on its account directory until it is closed, so a
/// second store over the same directory is refused at open, and two stores
/// that do coexist touch disjoint directories. A process-wide lock would only
/// serialise unrelated accounts behind one another's durable writes, each of
/// which costs two full flushes.
///
/// The [`DirectoryLock`] lives inside the mutex, so closing a store and
/// running an operation on it are ordered: an operation either finishes
/// before the directory is released or is refused after. Held apart, a write
/// already past its check could land in a directory that another store had
/// opened in between.
pub(super) struct StoreLock(Mutex<Option<DirectoryLock>>);

/// The lock for one operation. The store is open for as long as it is held.
pub(super) type StoreGuard<'a> = MutexGuard<'a, Option<DirectoryLock>>;

impl StoreLock {
    /// A lock over a store that holds `directory`.
    pub(super) fn new(directory: DirectoryLock) -> Self {
        Self(Mutex::new(Some(directory)))
    }

    /// Takes the lock for one operation, or `None` once the store is closed.
    ///
    /// A panic while it was held cannot have left a record torn (every write
    /// is a rename), so a poisoned lock is still safe to take.
    pub(super) fn open(&self) -> Option<StoreGuard<'_>> {
        let guard = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.is_some().then_some(guard)
    }

    /// Releases the directory, after any operation in flight. Every later
    /// [`Self::open`] is `None`, and closing twice is closing once.
    pub(super) fn close(&self) {
        let released = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        drop(released);
    }
}

/// An exclusive lock on a store's account directory, held until the store is
/// closed or dropped (or the process dies).
///
/// Two stores over one directory, in one process or two, would each write
/// ratchet state the other never sees, and the MLS state would diverge
/// without any error. The lock turns that into a refused open. It is
/// advisory: it orders stores that take it, which every built-in store does.
///
/// The lock is the open handle, not the file. It dies with the process, so a
/// lock file left behind by a crash is not stale and needs no cleanup, and
/// removing one while a store is open would let a second store in beside it.
#[derive(Debug)]
pub(super) struct DirectoryLock {
    _handle: File,
}

/// Why a [`DirectoryLock`] was not taken.
#[derive(Debug)]
pub(super) enum LockError {
    /// Another store holds the lock.
    Held,
    /// The lock file could not be created or opened.
    Io(io::Error),
}

impl DirectoryLock {
    /// Takes the lock file `name` in `directory`, without waiting.
    ///
    /// A filesystem that does not support locking at all (some network
    /// mounts) is not a reason to refuse the store: the lock is logged as
    /// unavailable and the store opens unguarded, as every binding's store
    /// does. Only a lock that another store holds is a refusal.
    pub(super) fn acquire(directory: &Path, name: &str) -> Result<Self, LockError> {
        let path = directory.join(name);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(FILE_MODE);
        }
        #[cfg(windows)]
        {
            // Read sharing only: a second store asks for write access, which
            // this handle does not share, so its open fails while this handle
            // lives, which is the lock. Windows releases it with the handle.
            // Sharing nothing would refuse even a reader that shares
            // everything. With read sharing, a reader that itself shares
            // write access (as backup tools usually do) can open the lock
            // file; one that shares only read access still cannot, because
            // this handle holds write access. The lock file is empty, so a
            // backup that skips it loses nothing.
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(WINDOWS_FILE_SHARE_READ);
        }
        let handle = match options.open(&path) {
            Ok(handle) => handle,
            #[cfg(windows)]
            Err(err) if err.raw_os_error() == Some(WINDOWS_SHARING_VIOLATION) => {
                return Err(LockError::Held)
            }
            Err(err) => return Err(LockError::Io(err)),
        };
        #[cfg(unix)]
        {
            use rustix::fs::{flock, FlockOperation};
            match flock(&handle, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => {}
                Err(errno) if errno == rustix::io::Errno::WOULDBLOCK => {
                    return Err(LockError::Held)
                }
                Err(errno) => tracing::warn!(
                    path = %path.display(),
                    error = %io::Error::from(errno),
                    "store directory cannot be locked on this filesystem; \
                     a second store over it will not be refused"
                ),
            }
        }
        Ok(Self { _handle: handle })
    }
}

/// `ERROR_SHARING_VIOLATION`: the file is open elsewhere without sharing.
#[cfg(windows)]
const WINDOWS_SHARING_VIOLATION: i32 = 32;

/// `FILE_SHARE_READ`: later opens may read the file, never write it.
#[cfg(windows)]
const WINDOWS_FILE_SHARE_READ: u32 = 0x0000_0001;

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
/// Owner-only is therefore a Unix property; on Windows the directory inherits
/// its parent's access list.
///
/// Every directory this call creates is flushed into its parent. A record's
/// own flush makes its entry durable in its directory, but a directory that
/// is itself only staged in its parent can vanish on power loss with every
/// acknowledged record inside it, which is the first write to every category.
pub(super) fn private_mkdir(directory: &Path) -> io::Result<()> {
    let created = missing_ancestors(directory);
    make_private_directory(directory)?;
    // Deepest first is not required: every directory in the list exists now,
    // and each flush makes one entry in one parent durable.
    for made in &created {
        if let Some(parent) = flush_target(made) {
            sync_directory(parent)?;
        }
    }
    Ok(())
}

/// The directory whose entry for `made` must be flushed: its parent.
///
/// A one-component relative path (`keys`) has the empty path as its parent,
/// and opening the empty path fails with "not found", which would fail the
/// first open of a store over a relative root and let the second succeed.
/// That parent is the working directory, so it is flushed as `.`.
fn flush_target(made: &Path) -> Option<&Path> {
    match made.parent()? {
        parent if parent.as_os_str().is_empty() => Some(Path::new(".")),
        parent => Some(parent),
    }
}

/// `directory` and each of its ancestors that does not exist yet.
fn missing_ancestors(directory: &Path) -> Vec<PathBuf> {
    directory
        .ancestors()
        .take_while(|path| !path.as_os_str().is_empty() && !path.exists())
        .map(Path::to_path_buf)
        .collect()
}

fn make_private_directory(directory: &Path) -> io::Result<()> {
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
/// On Unix a failed flush is an error: the entry it was meant to make durable
/// may not survive power loss, so the write that depends on it must not be
/// acknowledged. A filesystem that does not support flushing a directory at
/// all (`EINVAL`, `ENOTSUP`) is the one exception, because there the
/// file-level flush is already the strongest guarantee available.
///
/// Elsewhere it is best effort: a directory cannot be opened for a flush on
/// Windows, and the file-level flush is what that platform offers.
///
/// An error here arrives after the rename, unlink or creation it follows has
/// already taken effect. It means "not known to be durable", never "nothing
/// changed": a caller that reads the entry straight back may find it, and it
/// may or may not survive power loss.
pub(super) fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        match File::open(directory).and_then(|handle| handle.sync_all()) {
            Ok(()) => Ok(()),
            Err(err) if directory_flush_unsupported(&err) => Ok(()),
            Err(err) => Err(err),
        }
    }
    #[cfg(not(unix))]
    {
        if let Ok(handle) = File::open(directory) {
            let _ = handle.sync_all();
        }
        Ok(())
    }
}

/// Whether `err` says the filesystem cannot flush a directory at all, as
/// opposed to failing to flush this one.
#[cfg(unix)]
fn directory_flush_unsupported(err: &io::Error) -> bool {
    use rustix::io::Errno;
    err.raw_os_error().is_some_and(|code| {
        code == Errno::INVAL.raw_os_error() || code == Errno::NOTSUP.raw_os_error()
    })
}

/// Writes `bytes` to `target` atomically and durably: a fresh owner-only
/// temporary in the same directory, flushed, renamed over the target, and
/// the directory flushed. A reader sees the old record or the new one, never
/// a torn one, and an acknowledged write survives power loss.
///
/// An error from the final directory flush arrives after the rename, so the
/// new record may already be in place: see [`sync_directory`].
pub(super) fn write_atomic(directory: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let (temporary, mut handle) = create_temporary(directory)?;
    let result = (|| {
        handle.write_all(bytes)?;
        handle.sync_all()?;
        drop(handle);
        fs::rename(&temporary, target)?;
        sync_directory(directory)
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
///
/// An error from the directory flush arrives after the unlink, so the file
/// may already be gone: see [`sync_directory`].
pub(super) fn remove(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => {
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
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

/// Whether `directory` holds a regular file whose name starts with `prefix`.
/// Stops at the first one. A missing directory, or a file in its place,
/// holds none.
pub(super) fn holds_entry(directory: &Path, prefix: &str) -> io::Result<bool> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(false)
        }
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        // An entry whose type cannot be read is an error, never "not a
        // record": the caller decides from this whether a new identity may
        // start over the directory.
        if entry.file_name().to_string_lossy().starts_with(prefix) && entry.file_type()?.is_file() {
            return Ok(true);
        }
    }
    Ok(false)
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
        sweep_directory(directory, MAX_SWEEP_SCANNED, MAX_SWEEP_TEMPORARIES);
    }
}

/// Removes stale temporaries in `directory`, reading at most `max_scanned`
/// entries and examining at most `max_temporaries` temporaries.
fn sweep_directory(directory: &Path, max_scanned: usize, max_temporaries: usize) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let now = SystemTime::now();
    let mut temporaries = 0usize;
    for entry in entries.take(max_scanned).flatten() {
        if temporaries >= max_temporaries {
            break;
        }
        let name = entry.file_name();
        if !name.to_str().is_some_and(|n| n.starts_with(TEMP_PREFIX)) {
            continue;
        }
        temporaries += 1;
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

    /// Records ahead of an orphan in the listing must not hide it: the cap
    /// counts temporaries, so a directory larger than the cap still gets its
    /// orphans swept.
    #[test]
    fn records_ahead_of_an_orphan_do_not_hide_it_from_the_sweep() {
        let root = TempRoot::new("sweep-cap");
        let long_ago = SystemTime::now() - STALE_TEMPORARY_AGE - Duration::from_secs(60);
        for i in 0..32 {
            fs::write(root.path().join(format!("k_{i:02}")), b"x").expect("write");
        }
        let old = root.path().join(format!("{TEMP_PREFIX}old"));
        fs::write(&old, b"x").expect("write");
        File::options()
            .write(true)
            .open(&old)
            .and_then(|h| h.set_modified(long_ago))
            .expect("backdate");

        // A temporary cap of one, far below the 33 entries present.
        sweep_directory(root.path(), 1_000, 1);

        assert!(!old.exists(), "an orphan behind the records was not swept");
    }

    /// A second lock on one directory is refused while the first lives, in
    /// the same process as in another, and is free again once it drops.
    #[test]
    fn a_directory_lock_is_exclusive_until_dropped() {
        let root = TempRoot::new("dir-lock");
        let first = DirectoryLock::acquire(root.path(), "test.lock").expect("first lock");
        assert!(matches!(
            DirectoryLock::acquire(root.path(), "test.lock"),
            Err(LockError::Held)
        ));
        // A different lock name in the same directory is a different lock.
        let _other = DirectoryLock::acquire(root.path(), "other.lock").expect("other lock");
        drop(first);
        DirectoryLock::acquire(root.path(), "test.lock").expect("lock after release");
    }

    /// Every directory the call creates exists afterwards, parents included,
    /// and the list of what to flush names exactly those.
    #[test]
    fn missing_ancestors_names_only_what_did_not_exist() {
        let root = TempRoot::new("mkdir");
        let deep = root.path().join("a").join("b");
        assert_eq!(
            missing_ancestors(&deep),
            vec![deep.clone(), root.path().join("a")]
        );
        private_mkdir(&deep).expect("mkdir");
        assert!(deep.is_dir());
        assert!(missing_ancestors(&deep).is_empty());
    }

    /// The first open of a store over a relative root creates a directory
    /// whose parent is the empty path. That must be flushed as the working
    /// directory, not opened as "", which fails with "not found".
    #[test]
    fn a_top_level_relative_directory_flushes_the_working_directory() {
        assert_eq!(flush_target(Path::new("keys")), Some(Path::new(".")));
        assert_eq!(
            flush_target(Path::new("keys/account-x")),
            Some(Path::new("keys"))
        );
        assert_eq!(flush_target(Path::new("./keys")), Some(Path::new(".")));
        assert_eq!(flush_target(Path::new("/keys")), Some(Path::new("/")));
        assert_eq!(flush_target(Path::new("/")), None);
        assert!(sync_directory(Path::new(".")).is_ok());
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
