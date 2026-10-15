//! Staging files for atomic saves: a new, unpredictably named file beside the target, written,
//! flushed to disk and then renamed over it ([`write_atomic`]).

use std::path::{Path, PathBuf};

/// How many staging names [`create_staging`] tries. A random 64-bit name is only taken if someone
/// put a file there on purpose, so running out means refusing, not trying harder.
pub const STAGING_ATTEMPTS: usize = 16;

/// Where the suffix goes in a staging name. Each caller keeps the form it has always used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagingName {
    /// `.{name}.{suffix}.pdfcraft-tmp`, used by the automation tools.
    SuffixThenTag,
    /// `.{name}.pdfcraft-{suffix}.tmp`, used by the desktop app's save.
    TagThenSuffix,
}

/// Create a new, empty staging file in `dir` for the file `name`, one name per suffix. It is
/// opened with `create_new`, which fails if anything already has the name (a file, a hard link,
/// a symbolic link even when dangling, a folder), on Windows as everywhere else; such a name is
/// skipped, never opened, so a file planted at the staging path can't receive or redirect the
/// write. Pass [`staging_suffixes`]: fixed suffixes are for tests, and a predictable name can be
/// taken in advance to refuse every save.
pub fn create_staging(
    dir: &Path,
    name: &str,
    form: StagingName,
    suffixes: impl IntoIterator<Item = u64>,
) -> std::io::Result<(PathBuf, std::fs::File)> {
    create_staging_like(dir, name, form, suffixes, None)
}

/// [`create_staging`], created with the Windows attributes a save keeps from the file it
/// replaces (`like`): hidden, system and not-indexed. They can only be given when the file is
/// created, and the rename carries the staging file's own attributes over the original's.
fn create_staging_like(
    dir: &Path,
    name: &str,
    form: StagingName,
    suffixes: impl IntoIterator<Item = u64>,
    like: Option<&std::fs::Metadata>,
) -> std::io::Result<(PathBuf, std::fs::File)> {
    // At most 128 bytes of the target's name, cut between characters, so the staging name fits
    // the 255-byte (Linux, macOS) and 255-unit (Windows) limits however long that name is.
    let mut stem = String::new();
    for c in name.chars() {
        if stem.len() + c.len_utf8() > 128 {
            break;
        }
        stem.push(c);
    }
    for suffix in suffixes.into_iter().take(STAGING_ATTEMPTS) {
        let tmp = dir.join(match form {
            StagingName::SuffixThenTag => format!(".{stem}.{suffix:016x}.pdfcraft-tmp"),
            StagingName::TagThenSuffix => format!(".{stem}.pdfcraft-{suffix:016x}.tmp"),
        });
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(windows)]
        if let Some(like) = like {
            use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
            const KEPT: u32 = 0x2 | 0x4 | 0x2000; // HIDDEN | SYSTEM | NOT_CONTENT_INDEXED
            options.attributes(like.file_attributes() & KEPT);
        }
        #[cfg(not(windows))]
        let _ = like;
        match options.open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            // Windows reports a folder at the name as "access denied"; it is taken all the same.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && tmp.symlink_metadata().is_ok() => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "every temporary file name tried is taken"))
}

/// Unpredictable staging-name suffixes, so a name can't be planted in advance. Each is the
/// standard library's keyed hash of a counter (`RandomState::hash_one`, currently SipHash-1-3),
/// keyed from the operating system's random source. That is all a staging name needs, but it is
/// not a CSPRNG: don't use these where a value must stay secret or unguessable after it has been
/// seen, such as a token or a key.
pub fn staging_suffixes() -> impl Iterator<Item = u64> {
    use std::hash::BuildHasher;
    let state = std::hash::RandomState::new();
    (0u64..).map(move |i| state.hash_one(i))
}

/// Write `bytes` to `path` atomically: into a new staging file beside it ([`create_staging`]),
/// flushed to disk so a crash right after the rename can't leave a truncated file, then renamed
/// over `path`. On failure the staging file is removed and `path` is unchanged.
///
/// Replacing an existing file keeps what belongs to it rather than to its contents:
/// - a symbolic link stays a link, and the file it points to is written;
/// - its Windows hidden/system attributes, and its permissions on Unix;
/// - a read-only file is refused with a message saying so, not "access denied".
pub fn write_atomic(path: &Path, bytes: &[u8], form: StagingName, suffixes: impl IntoIterator<Item = u64>) -> std::io::Result<()> {
    write_atomic_syncing(path, bytes, form, suffixes, std::fs::File::sync_all)
}

/// [`write_atomic`], flushing the staging file with `sync_all` (the standard library's in use, a
/// failing one in tests).
fn write_atomic_syncing(
    path: &Path,
    bytes: &[u8],
    form: StagingName,
    suffixes: impl IntoIterator<Item = u64>,
    sync_all: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::io::Write;
    // Through a symbolic link: replace the file it points to, not the link (a dangling link is
    // replaced, as there is nothing behind it to keep).
    let resolved;
    let path = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => match std::fs::canonicalize(path) {
            Ok(target) => {
                resolved = target;
                resolved.as_path()
            }
            Err(_) => path,
        },
        _ => path,
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "save".into());
    let existing = std::fs::metadata(path).ok();
    if existing.as_ref().is_some_and(|m| m.permissions().readonly()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{name} is read-only: save it under another name, or clear its read-only setting first"),
        ));
    }
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let (tmp, file) = create_staging_like(dir, &name, form, suffixes, existing.as_ref())?;
    let result = (|| {
        // Closed at the end of the block, before the rename.
        {
            let mut file = file;
            file.write_all(bytes)?;
            sync_staged(&file, sync_all)?;
        }
        #[cfg(unix)]
        if let Some(m) = &existing {
            std::fs::set_permissions(&tmp, m.permissions())?;
        }
        replace(&tmp, path, &name)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Rename the staging file over the target. On Windows, a scanner or indexer that opened the
/// new file a moment ago can hold it briefly, so a refused rename is retried for up to half a
/// second; a program that keeps the target open gets a message saying so.
fn replace(tmp: &Path, path: &Path, name: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    for _ in 0..10 {
        match std::fs::rename(tmp, path) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => std::thread::sleep(std::time::Duration::from_millis(50)),
            other => return other,
        }
    }
    std::fs::rename(tmp, path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            std::io::Error::new(
                e.kind(),
                format!("couldn't replace {name}: another program has it open, or you aren't allowed to change it. Close it there and save again, or save under another name"),
            )
        } else {
            e
        }
    })
}

/// Flush the staging file to the disk before it replaces the original. On macOS `sync_all` asks
/// for a full flush (`F_FULLFSYNC`), which SMB and FUSE volumes, such as a mounted Cryptomator
/// vault, refuse as unsupported: there a plain `fsync` is the strongest flush the volume offers
/// (upstream #981). Any other failure still fails the save.
fn sync_staged(f: &std::fs::File, sync_all: impl FnOnce(&std::fs::File) -> std::io::Result<()>) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use rustix::io::Errno;
        match sync_all(f) {
            Err(e) if matches!(Errno::from_io_error(&e), Some(Errno::NOTSUP | Errno::NOTTY)) => Ok(rustix::fs::fsync(f)?),
            other => other,
        }
    }
    #[cfg(not(target_os = "macos"))]
    sync_all(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty folder for one test.
    fn test_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-platform-staging-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_name(p: &Path) -> String {
        p.file_name().unwrap().to_string_lossy().into_owned()
    }

    /// The callers' tests plant files at these exact names; if a name changed, those tests would
    /// plant where nothing writes and pass without testing anything.
    #[test]
    fn each_form_keeps_its_exact_name() {
        let dir = test_dir("forms");
        let (tools, _) = create_staging(&dir, "out.pdf", StagingName::SuffixThenTag, [0xab]).unwrap();
        assert_eq!(file_name(&tools), ".out.pdf.00000000000000ab.pdfcraft-tmp");
        let (desktop, _) = create_staging(&dir, "out.pdf", StagingName::TagThenSuffix, [0xab]).unwrap();
        assert_eq!(file_name(&desktop), ".out.pdf.pdfcraft-00000000000000ab.tmp");
        assert_eq!(std::fs::read(&tools).unwrap(), b"", "a new, empty file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_name_is_cut_to_128_bytes_between_characters() {
        let dir = test_dir("long");
        // One byte, then four-byte characters: the 128-byte limit falls inside the 32nd one.
        let name = format!("a{}.pdf", "\u{1F600}".repeat(40));
        let (tmp, _) = create_staging(&dir, &name, StagingName::SuffixThenTag, [1]).unwrap();
        assert_eq!(file_name(&tmp), format!(".a{}.0000000000000001.pdfcraft-tmp", "\u{1F600}".repeat(31)));
        // One-byte characters: exactly 128 of them are kept.
        let (tmp, _) = create_staging(&dir, &"x".repeat(200), StagingName::SuffixThenTag, [2]).unwrap();
        assert_eq!(file_name(&tmp), format!(".{}.0000000000000002.pdfcraft-tmp", "x".repeat(128)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_out_of_suffixes_refuses_and_creates_nothing() {
        let dir = test_dir("empty");
        let e = create_staging(&dir, "out.pdf", StagingName::TagThenSuffix, []).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_first_sixteen_names_are_tried() {
        let dir = test_dir("cap");
        let name = |s: u64| dir.join(format!(".out.pdf.{s:016x}.pdfcraft-tmp"));
        for s in 1..=STAGING_ATTEMPTS as u64 {
            std::fs::write(name(s), "PLANTED").unwrap();
        }
        assert!(create_staging(&dir, "out.pdf", StagingName::SuffixThenTag, 1..).is_err());
        assert!(!name(STAGING_ATTEMPTS as u64 + 1).exists(), "a 17th name was tried");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    fn save(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        write_atomic(path, bytes, StagingName::TagThenSuffix, staging_suffixes())
    }

    #[test]
    fn a_read_only_file_is_refused_with_a_message_and_left_alone() {
        let dir = test_dir("readonly");
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        let mut perms = std::fs::metadata(&target).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&target, perms.clone()).unwrap();
        let e = save(&target, b"NEW").unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(e.to_string().contains("read-only"), "{e}");
        assert_eq!(std::fs::read(&target).unwrap(), b"OLD");
        assert_eq!(listing(&dir), ["out.pdf"], "no staging file is left behind");
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&target, perms).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Saving over a hidden file kept its contents but made it visible: the rename carried the
    /// new file's attributes over.
    #[cfg(windows)]
    #[test]
    fn a_hidden_file_stays_hidden() {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        let dir = test_dir("hidden");
        let target = dir.join("out.pdf");
        std::fs::OpenOptions::new().write(true).create_new(true).attributes(0x2).open(&target).unwrap();
        save(&target, b"NEW").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        assert_ne!(std::fs::metadata(&target).unwrap().file_attributes() & 0x2, 0, "still hidden");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A program holding the file open without letting it be replaced: the save fails with a
    /// message that says so, and the original is untouched.
    #[cfg(windows)]
    #[test]
    fn a_file_open_elsewhere_says_so() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = test_dir("inuse");
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0x1).open(&target).unwrap(); // FILE_SHARE_READ only
        let e = save(&target, b"NEW").unwrap_err();
        drop(held);
        assert!(e.to_string().contains("another program has it open"), "{e}");
        assert_eq!(std::fs::read(&target).unwrap(), b"OLD");
        assert_eq!(listing(&dir), ["out.pdf"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Saving through a symbolic link writes the file it points to and leaves the link a link.
    /// (Creating a link on Windows needs Developer Mode or an administrator; skipped without.)
    #[test]
    fn a_symbolic_link_stays_a_link_and_its_target_is_written() {
        let dir = test_dir("symlink");
        let real = dir.join("real.pdf");
        let link = dir.join("link.pdf");
        std::fs::write(&real, "OLD").unwrap();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&real, &link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&real, &link);
        if made.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        save(&link, b"NEW").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "still a link");
        assert_eq!(std::fs::read(&real).unwrap(), b"NEW");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn permissions_are_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = test_dir("mode");
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        save(&target, b"NEW").unwrap();
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failed flush fails the save and keeps the original.
    #[test]
    fn a_failed_flush_keeps_the_original() {
        let dir = test_dir("syncfails");
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        let e = write_atomic_syncing(&target, b"NEW", StagingName::TagThenSuffix, [7], |_| Err(std::io::Error::other("disk gone"))).unwrap_err();
        assert_eq!(e.to_string(), "disk gone");
        assert_eq!(std::fs::read(&target).unwrap(), b"OLD");
        assert_eq!(listing(&dir), ["out.pdf"], "the staging file was removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// On macOS `sync_all` asks for a full flush, which SMB and FUSE volumes (a Cryptomator vault,
    /// upstream #981) refuse as unsupported: ENOTSUP (45) or ENOTTY (25). The save still goes through.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_volume_without_full_flush_still_saves() {
        for errno in [45, 25] {
            let dir = test_dir(&format!("nofullsync-{errno}"));
            let target = dir.join("out.pdf");
            std::fs::write(&target, "OLD").unwrap();
            let asked = std::cell::Cell::new(false);
            let refuse = |_: &std::fs::File| {
                asked.set(true);
                Err(std::io::Error::from_raw_os_error(errno))
            };
            write_atomic_syncing(&target, b"NEW", StagingName::TagThenSuffix, [7], refuse).unwrap();
            assert!(asked.get(), "the full flush was tried");
            assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
            assert_eq!(listing(&dir), ["out.pdf"], "no staging file is left behind");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
