//! Per-instance ownership of one untitled recovery slot (Pulse #3948).
//!
//! Each running hex instance holds an exclusive advisory lock on a `.owner`
//! file beside its untitled journal for as long as its claim lives, so two
//! instances never checkpoint into the same slot. The operating system releases
//! the lock when the process exits or crashes; the journal it leaves behind is
//! then claimable by the next instance, which offers it through the existing
//! leftover-recovery path. Slot 0 keeps the historical `untitled.hex-journal`
//! name; later slots are `untitled-2.hex-journal`, `untitled-3.hex-journal`...
//!
//! Owner files are never removed: they hold no data and removing a lock file
//! another process may be opening would break mutual exclusion.
//! A hex build from before this contract does not take owner locks.
use crate::StorageError;
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

/// Bound on concurrently owned untitled slots in one recovery directory.
pub const MAX_UNTITLED_SLOTS: usize = 64;
const OWNER_SUFFIX: &str = ".owner";

/// Exclusive ownership of one untitled recovery slot. Dropping it releases the
/// slot; keep it alive for as long as anything checkpoints into [`Self::path`].
#[derive(Debug)]
pub struct UntitledSlotClaim {
    path: PathBuf,
    _owner: File,
}

impl UntitledSlotClaim {
    /// Claim a slot in an existing recovery `directory`. A slot left behind by
    /// an ended instance (its journal exists but nobody owns it) is preferred,
    /// lowest index first, so its leftover work is offered for recovery;
    /// otherwise the lowest-index empty slot is claimed. Slots owned by a live
    /// instance are never claimed.
    pub fn claim(directory: &Path) -> Result<Self, StorageError> {
        let slots: Vec<PathBuf> = (0..MAX_UNTITLED_SLOTS)
            .map(|index| untitled_slot_path(directory, index))
            .collect();
        let (leftovers, empty): (Vec<&PathBuf>, Vec<&PathBuf>) = slots
            .iter()
            .partition(|path| fs::symlink_metadata(path).is_ok());
        for path in leftovers.into_iter().chain(empty) {
            if let Some(owner) = try_own(path)? {
                return Ok(Self {
                    path: path.clone(),
                    _owner: owner,
                });
            }
        }
        Err(StorageError::Invalid(
            "every untitled recovery slot is owned by another running hex; close one and retry",
        ))
    }

    /// Claim every other slot in `directory` whose journal an ended instance
    /// left behind (Pulse #4575), lowest index first, so one launch can offer
    /// all of them. `own` is skipped; slots a live instance owns are never
    /// claimed. A slot whose owner entry cannot be locked (for example a
    /// symbolic link in its place) is left untouched and not returned.
    /// Holding a returned claim keeps other launching instances from adopting
    /// or offering the same leftover; drop it once its offer is resolved.
    pub fn claim_leftovers(directory: &Path, own: &Path) -> Vec<Self> {
        (0..MAX_UNTITLED_SLOTS)
            .map(|index| untitled_slot_path(directory, index))
            .filter(|path| path != own && fs::symlink_metadata(path).is_ok())
            .filter_map(|path| match try_own(&path) {
                Ok(Some(owner)) => Some(Self {
                    path,
                    _owner: owner,
                }),
                Ok(None) | Err(_) => None,
            })
            .collect()
    }

    /// The untitled journal this claim exclusively owns.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The journal path of untitled slot `index` inside `directory`.
pub fn untitled_slot_path(directory: &Path, index: usize) -> PathBuf {
    match index {
        0 => directory.join("untitled.hex-journal"),
        index => directory.join(format!("untitled-{}.hex-journal", index.saturating_add(1))),
    }
}

/// Acquire the slot's owner lock, or `None` while another live owner holds it.
fn try_own(journal: &Path) -> Result<Option<File>, StorageError> {
    let mut name = journal
        .file_name()
        .ok_or(StorageError::Invalid("untitled slot has no file name"))?
        .to_os_string();
    name.push(OWNER_SUFFIX);
    let owner = journal.with_file_name(name);
    match fs::symlink_metadata(&owner) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(StorageError::Invalid(
                "untitled slot owner must be a regular non-symbolic file",
            ));
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(owner)?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(file)),
        Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Journal, load_journal};
    use hex_project::Project;
    use std::{
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-untitled-slot-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("isolated directory");
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn child(directory: &Path, rate: &str) -> String {
        let output = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "journal::untitled_slot::tests::slot_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("HEX_UNTITLED_SLOT_DIR", directory)
            .env("HEX_UNTITLED_SLOT_RATE", rate)
            .output()
            .expect("child instance");
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        stdout
            .lines()
            .find_map(|line| line.split_once("claimed=").map(|(_, path)| path))
            .expect("child reports its slot")
            .to_owned()
    }

    /// Regression for #3948: two live instances each with an untitled project
    /// used to share one slot, so the last writer replaced the other's work.
    #[test]
    fn separate_processes_keep_separate_untitled_checkpoints() {
        let dir = Temp::new();
        let parent = UntitledSlotClaim::claim(&dir.0).expect("first instance");
        assert_eq!(parent.path(), untitled_slot_path(&dir.0, 0));
        let mine = Project::new(48_000).expect("project");
        Journal::open_empty(parent.path().to_path_buf())
            .expect("own slot")
            .checkpoint(&mine)
            .expect("first instance checkpoint");

        // The second instance runs and exits (as a crash would) while the
        // first still owns slot 0; its work lands in its own slot.
        let other = child(&dir.0, "44100");
        assert_eq!(Path::new(&other), untitled_slot_path(&dir.0, 1));
        assert_eq!(load_journal(parent.path()).expect("mine kept"), mine);
        assert_eq!(
            load_journal(Path::new(&other)).expect("other kept"),
            Project::new(44_100).expect("project")
        );

        // A relaunch adopts the ended instance's leftover, never the live slot.
        let relaunch = UntitledSlotClaim::claim(&dir.0).expect("relaunch");
        assert_eq!(relaunch.path(), Path::new(&other));
        // With both owned, a third instance takes the next empty slot.
        let third = UntitledSlotClaim::claim(&dir.0).expect("third");
        assert_eq!(third.path(), untitled_slot_path(&dir.0, 2));
        assert!(!third.path().exists());
        drop(relaunch);
        let again = UntitledSlotClaim::claim(&dir.0).expect("released slot");
        assert_eq!(again.path(), Path::new(&other));
    }

    #[test]
    fn slot_child() {
        let Some(directory) = std::env::var_os("HEX_UNTITLED_SLOT_DIR") else {
            return;
        };
        let rate: u32 = std::env::var("HEX_UNTITLED_SLOT_RATE")
            .expect("rate")
            .parse()
            .expect("numeric rate");
        let claim = UntitledSlotClaim::claim(Path::new(&directory)).expect("child claim");
        Journal::open_empty(claim.path().to_path_buf())
            .expect("child slot")
            .checkpoint(&Project::new(rate).expect("project"))
            .expect("child checkpoint");
        println!("claimed={}", claim.path().display());
    }

    #[cfg(unix)]
    #[test]
    fn foreign_owner_entries_are_refused_and_preserved() {
        let dir = Temp::new();
        let owner = dir.0.join("untitled.hex-journal.owner");
        std::os::unix::fs::symlink(dir.0.join("elsewhere"), &owner).expect("symlink");
        assert!(matches!(
            UntitledSlotClaim::claim(&dir.0),
            Err(StorageError::Invalid(message)) if message.contains("non-symbolic")
        ));
        assert!(fs::symlink_metadata(&owner).expect("kept").is_symlink());
        assert!(!dir.0.join("elsewhere").exists());
    }

    /// Regression for #4575: two ended instances left journals; one launch
    /// claims both leftovers, never the live-owned slot nor its own.
    #[test]
    fn one_launch_claims_every_unowned_leftover_but_no_live_slot() {
        let dir = Temp::new();
        let live = UntitledSlotClaim::claim(&dir.0).expect("live instance");
        assert_eq!(live.path(), untitled_slot_path(&dir.0, 0));
        Journal::open_empty(live.path().to_path_buf())
            .expect("live slot")
            .checkpoint(&Project::new(48_000).expect("project"))
            .expect("live checkpoint");
        let ended = child(&dir.0, "44100");
        assert_eq!(Path::new(&ended), untitled_slot_path(&dir.0, 1));
        // Two more ended instances left their journals in slots 3 and 4.
        let leftovers = [untitled_slot_path(&dir.0, 3), untitled_slot_path(&dir.0, 4)];
        for leftover in &leftovers {
            Journal::open_empty(leftover.clone())
                .expect("ended slot")
                .checkpoint(&Project::new(44_100).expect("project"))
                .expect("ended checkpoint");
        }

        let own = UntitledSlotClaim::claim(&dir.0).expect("relaunch");
        assert_eq!(own.path(), Path::new(&ended));
        let others = UntitledSlotClaim::claim_leftovers(&dir.0, own.path());
        let paths: Vec<&Path> = others.iter().map(UntitledSlotClaim::path).collect();
        assert_eq!(
            paths,
            leftovers.iter().map(PathBuf::as_path).collect::<Vec<_>>()
        );
        // Held leftovers are not adopted by another launching instance.
        let next = UntitledSlotClaim::claim(&dir.0).expect("next instance");
        assert_eq!(next.path(), untitled_slot_path(&dir.0, 2));
        assert!(UntitledSlotClaim::claim_leftovers(&dir.0, next.path()).is_empty());
        drop(others);
        let released = UntitledSlotClaim::claim_leftovers(&dir.0, next.path());
        let paths: Vec<&Path> = released.iter().map(UntitledSlotClaim::path).collect();
        assert_eq!(
            paths,
            leftovers.iter().map(PathBuf::as_path).collect::<Vec<_>>()
        );
        // Nothing was removed or rewritten by claiming.
        for leftover in &leftovers {
            assert_eq!(
                load_journal(leftover).expect("kept"),
                Project::new(44_100).expect("project")
            );
        }
    }

    #[test]
    fn exhausted_slots_report_a_next_step() {
        let dir = Temp::new();
        let claims: Vec<_> = (0..MAX_UNTITLED_SLOTS)
            .map(|_| UntitledSlotClaim::claim(&dir.0).expect("claim"))
            .collect();
        let error = UntitledSlotClaim::claim(&dir.0).expect_err("bounded");
        assert!(error.to_string().contains("close one and retry"));
        drop(claims);
    }
}
