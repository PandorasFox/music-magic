//! File Operations
//!
//! Handles execution of file-related mutations:
//! - Move: Move a file from source to destination
//! - MoveToStash: Move a file to the stash directory
//! - HardLink: Create a hard link (for deployment)
//! - LibraryMove: Move a file within a library

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::meta::computations::{derivation, Computation};
use crate::meta::recomputation::RecomputationScope;
use crate::meta::signals::data::*;

use super::traits::{MutationContext, MutationExecutor};
use super::types::{Mutation, MutationResult, SignalClearScope, SignalToClear};

// Re-export struct definitions from mm-meta
pub use mm_meta::mutations::file_ops::{
    HardLinkMutation, LibraryMoveMutation, MoveMutation,
    StashFromZoneMutation, StashLeftoversMutation,
};

// ============================================================================
// MutationExecutor Implementations
// ============================================================================

impl MutationExecutor for MoveMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::Staged
    }
    fn execution_stage(&self) -> super::MutationExecutionStage {
        super::MutationExecutionStage::DiskFlush
    }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let result = execute_move(&self.source, &self.destination);
        MutationResult::from_unit_result(Mutation::Move(self.clone()), result, start)
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::MutableOnly
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }
    fn recomputation_scope(&self) -> RecomputationScope {
        RecomputationScope::FILES
    }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        vec![self.source.clone(), self.destination.clone()]
    }
}

impl MutationExecutor for StashFromZoneMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::Staged
    }
    fn execution_stage(&self) -> super::MutationExecutionStage {
        super::MutationExecutionStage::DiskFlush
    }

    fn execute(&self, ctx: &MutationContext) -> MutationResult {
        use std::os::unix::fs::MetadataExt;

        let start = std::time::Instant::now();

        // Discover inode before the move (file still exists at original path)
        let discovered = std::fs::metadata(&self.path)
            .map(|m| vec![m.ino() as i64])
            .unwrap_or_default();

        let stash_root = ctx.snapshot.config.as_ref().map(|c| c.stash_dir());
        let result = match stash_root {
            Some(ref root) => execute_move_to_stash(&self.path, &self.stash_name, root),
            None => Err(anyhow::anyhow!(
                "Stash directory not configured. File: {}",
                self.path.display()
            )),
        };
        let (success, error) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(format!("{:#}", e))),
        };
        MutationResult {
            _mutation: Mutation::StashFromZone(self.clone()),
            success,
            error,
            _duration_ms: start.elapsed().as_millis() as u64,
            spawn_mutations: Vec::new(),
            pending_signals: Vec::new(),
            discovered_inodes: discovered,
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::All
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }
    fn recomputation_scope(&self) -> RecomputationScope {
        RecomputationScope::FILES | RecomputationScope::DEPLOY
    }
}

impl MutationExecutor for StashLeftoversMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::Staged
    }
    fn execution_stage(&self) -> super::MutationExecutionStage {
        super::MutationExecutionStage::DiskFlush
    }

    fn execute(&self, ctx: &MutationContext) -> MutationResult {
        use std::os::unix::fs::MetadataExt;

        let start = std::time::Instant::now();

        // Discover inode before the move (file still exists at original path)
        let discovered = std::fs::metadata(&self.path)
            .map(|m| vec![m.ino() as i64])
            .unwrap_or_default();

        let stash_root = ctx.snapshot.config.as_ref().map(|c| c.stash_dir());
        let result = match stash_root {
            Some(ref root) => execute_move_to_stash(&self.path, "library_leftovers", root),
            None => Err(anyhow::anyhow!(
                "Stash directory not configured. File: {}",
                self.path.display()
            )),
        };
        let (success, error) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(format!("{:#}", e))),
        };
        MutationResult {
            _mutation: Mutation::StashLeftovers(self.clone()),
            success,
            error,
            _duration_ms: start.elapsed().as_millis() as u64,
            spawn_mutations: Vec::new(),
            pending_signals: Vec::new(),
            discovered_inodes: discovered,
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::All
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }
    fn recomputation_scope(&self) -> RecomputationScope {
        RecomputationScope::FILES | RecomputationScope::DEPLOY
    }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        use crate::corpus::paths;
        use crate::db::types::Zone;
        let resolver = paths::get_resolver();
        if let Some(lib_rel) = resolver.to_zone_relative(&self.path, Zone::Library) {
            let library_path = lib_rel.to_string_lossy();
            if let Some(library_name) = library_path.split('/').next() {
                let key = LibraryLeftoverSignal::make_key(library_name, &library_path);
                return vec![SignalToClear::exact::<LibraryLeftoverSignal>(key)];
            }
        }
        Vec::new()
    }
}

impl MutationExecutor for HardLinkMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::Staged
    }
    fn execution_stage(&self) -> super::MutationExecutionStage {
        super::MutationExecutionStage::DiskDeploy
    }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let result = execute_hard_link(&self.source, &self.destination);
        MutationResult::from_unit_result(Mutation::HardLink(self.clone()), result, start)
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::MutableOnly
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }
    fn recomputation_scope(&self) -> RecomputationScope {
        RecomputationScope::DEPLOY
    }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        vec![self.source.clone(), self.destination.clone()]
    }

    fn additional_computations(&self) -> Vec<Computation> {
        vec![Computation::Derivation(
            derivation::Computation::UpdateDeploySignals {
                corpus_path: self.source.clone(),
                library_path: self.destination.clone(),
            },
        )]
    }
}

impl MutationExecutor for LibraryMoveMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::Staged
    }
    fn execution_stage(&self) -> super::MutationExecutionStage {
        super::MutationExecutionStage::DiskDeploy
    }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let result = execute_library_move_impl(&self.source, &self.destination);
        MutationResult::from_unit_result(Mutation::LibraryMove(self.clone()), result, start)
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::MutableOnly
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }
    fn recomputation_scope(&self) -> RecomputationScope {
        RecomputationScope::DEPLOY
    }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        vec![self.source.clone(), self.destination.clone()]
    }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        use crate::corpus::paths;
        use crate::db::types::Zone;
        let resolver = paths::get_resolver();
        if let Some(lib_rel) = resolver.to_zone_relative(&self.source, Zone::Library) {
            let library_path = lib_rel.to_string_lossy();
            if let Some(library_name) = library_path.split('/').next() {
                let key = LibraryStaleSignal::make_key(library_name, &library_path);
                return vec![SignalToClear::exact::<LibraryStaleSignal>(key)];
            }
        }
        Vec::new()
    }
}

// ============================================================================
// Execution Helpers (unchanged filesystem operations)
// ============================================================================

/// Execute a Move mutation.
///
/// Moves a file from source to destination, creating parent directories if needed.
pub fn execute_move(source: &Path, destination: &Path) -> Result<()> {
    // Ensure source exists
    if !source.exists() {
        return Err(anyhow::anyhow!(
            "Source file does not exist: {}",
            source.display()
        ));
    }

    // Create destination directory if needed
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Move the file
    fs::rename(source, destination).with_context(|| {
        format!(
            "Failed to move {} to {}",
            source.display(),
            destination.display()
        )
    })?;

    Ok(())
}

/// Execute a HardLink mutation.
///
/// Creates a hard link from source to destination (for deployment).
pub fn execute_hard_link(source: &Path, destination: &Path) -> Result<()> {
    // Ensure source exists
    if !source.exists() {
        return Err(anyhow::anyhow!(
            "Source file does not exist: {}",
            source.display()
        ));
    }

    // Create destination directory if needed
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // If destination already exists, check if it's the same inode (already deployed).
    // This handles ghost hardlinks: files on disk but missing from the DB index.
    if destination.exists() {
        use std::os::unix::fs::MetadataExt;
        let src_ino = fs::metadata(source)
            .with_context(|| format!("Failed to stat source: {}", source.display()))?
            .ino();
        let dst_ino = fs::metadata(destination)
            .with_context(|| format!("Failed to stat destination: {}", destination.display()))?
            .ino();
        if src_ino == dst_ino {
            // Same inode — already correctly deployed, just not indexed.
            return Ok(());
        }
        return Err(anyhow::anyhow!(
            "Destination already exists (different inode): {}",
            destination.display()
        ));
    }

    // Create hard link
    fs::hard_link(source, destination).with_context(|| {
        format!(
            "Failed to create hard link from {} to {}",
            source.display(),
            destination.display()
        )
    })?;

    Ok(())
}

/// Execute a LibraryMove mutation.
///
/// Moves a file within a library (e.g., stale file to correct location).
/// Unlike corpus moves, this does not update any database records.
///
/// IMPORTANT: MM never unlinks/destroys data. If destination exists:
/// - Same inode: File already correctly deployed, nothing to do (caller should
///   handle stale source path via separate MoveToStash if cleanup needed)
/// - Different inode: Conflict - fails so caller can stash the conflicting file first
pub fn execute_library_move_impl(source: &Path, destination: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    // Check if destination already exists
    if destination.exists() {
        let src_inode = source.metadata()?.ino();
        let dst_inode = destination.metadata()?.ino();

        if src_inode == dst_inode {
            // Same file already at destination - nothing to do
            // The source is a stale hard link; if cleanup is desired,
            // caller should issue a separate MoveToStash for the source path
            return Ok(());
        } else {
            // Different file at destination - refuse to clobber
            // Caller must stash the conflicting destination file first
            return Err(anyhow::anyhow!(
                "Destination already exists with different inode: {} (source inode: {}, dest inode: {}). \
                 Stash the conflicting file first, then retry.",
                destination.display(),
                src_inode,
                dst_inode
            ));
        }
    }

    // Create parent directories if needed
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Move the file
    fs::rename(source, destination).with_context(|| {
        format!(
            "Failed to move library file from {} to {}",
            source.display(),
            destination.display()
        )
    })?;

    Ok(())
}

/// Maximum bytes for a single filename component (ext4/NAME_MAX).
const STASH_FILENAME_MAX_BYTES: usize = 255;

/// Cap on stash-slot uniquifying attempts.
///
/// Reaching this does not mean "we ran out of names" — the `_N` suffix has
/// effectively unlimited room. It means something upstream is re-creating the
/// source path on every cycle (deploy/leftover flapping), and the stash is the
/// symptom rather than the problem. Fail loudly instead of stat(2)-ing forever.
const STASH_UNIQUIFY_MAX_ATTEMPTS: usize = 1000;

/// Return true if `a` and `b` resolve to the same inode. False if either
/// path can't be stat'd or the inodes differ.
fn same_inode(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(ma), Ok(mb)) => ma.ino() == mb.ino() && ma.dev() == mb.dev(),
        _ => false,
    }
}

/// Pick the stash path to rename `source` onto: `preferred` if free, otherwise
/// `stem_2.ext`, `stem_3.ext`, … until a free name is found.
///
/// Returns `Ok(None)` when `preferred` *is* `source` — the file already occupies its
/// own stash slot, so there is nothing to vacate.
///
/// A candidate that exists but shares `source`'s inode is skipped, not accepted as a
/// no-op. Deploy hardlinks corpus files into the library, so a library link and a
/// stash entry routinely share an inode; the data being preserved already is not a
/// reason to leave the source link in place. Every caller stashes precisely to free
/// the source path, and reporting success without freeing it deadlocks the deploy
/// pipeline — the DeployLink waiting on that library slot fails with "Destination
/// already exists (different inode)" on every cycle, forever. The extra stash entries
/// this creates are directory entries against an existing inode: no storage cost, and
/// nothing is ever unlinked.
///
/// The suffix counts rather than repeats: an earlier unary scheme (`track_.flac`,
/// `track__.flac`, …) grew the filename by a byte per collision, so a slot that had
/// flapped a couple hundred times hit NAME_MAX and could never be vacated again.
fn resolve_stash_destination(source: &Path, preferred: PathBuf) -> Result<Option<PathBuf>> {
    if !preferred.exists() {
        return Ok(Some(preferred));
    }
    if preferred == source {
        return Ok(None);
    }

    let mut same_inode_collisions = usize::from(same_inode(source, &preferred));

    let parent = preferred.parent().map(PathBuf::from);
    let stem = preferred
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    let extension = preferred.extension().map(|e| e.to_os_string());
    let dot_ext_len = extension.as_ref().map(|e| e.len() + 1).unwrap_or(0);

    for n in 2..=STASH_UNIQUIFY_MAX_ATTEMPTS {
        let suffix = format!("_{}", n);
        if stem.len() + suffix.len() + dot_ext_len > STASH_FILENAME_MAX_BYTES {
            return Err(anyhow::anyhow!(
                "Cannot uniquify stash filename for {}: NAME_MAX ({} bytes) exceeded at \
                 suffix {} in {}",
                source.display(),
                STASH_FILENAME_MAX_BYTES,
                suffix,
                parent.map(|p| p.display().to_string()).unwrap_or_default(),
            ));
        }

        let mut filename = stem.clone();
        filename.push(&suffix);
        if let Some(ref ext) = extension {
            filename.push(".");
            filename.push(ext);
        }
        let dest = match &parent {
            Some(p) => p.join(&filename),
            None => PathBuf::from(&filename),
        };

        if !dest.exists() {
            if same_inode_collisions > 0 {
                crate::logging::log_general(format!(
                    "[STASH] {} already has {} stash entr{} sharing its inode; \
                     vacating the source link to {}",
                    source.display(),
                    same_inode_collisions,
                    if same_inode_collisions == 1 { "y" } else { "ies" },
                    dest.display(),
                ));
            }
            return Ok(Some(dest));
        }
        if dest == source {
            return Ok(None);
        }
        if same_inode(source, &dest) {
            same_inode_collisions += 1;
        }
    }

    Err(anyhow::anyhow!(
        "Cannot uniquify stash filename for {}: {} occupied slots in {}. \
         Something is re-creating this path every cycle — fix the deploy/leftover \
         flapping upstream rather than the stash.",
        source.display(),
        STASH_UNIQUIFY_MAX_ATTEMPTS,
        parent.map(|p| p.display().to_string()).unwrap_or_default(),
    ))
}

/// Move a file to the stash directory, preserving corpus/library directory structure.
///
/// Destination: `{stash_root}/{stash_name}/{relative_path}`
/// where `relative_path` is the path within corpus/ or libraries/, preserving structure.
///
/// Destination selection (underscore uniquifying, same-inode handling, NAME_MAX
/// bound) lives in `resolve_stash_destination`. On success `path` is always vacated.
///
/// Examples:
/// - `/archive/corpus/Artist/Album/track.flac` -> `stash/overlaps/Artist/Album/track.flac`
/// - `/archive/libraries/music/Artist/track.mp3` -> `stash/leftovers/music/Artist/track.mp3`
pub fn execute_move_to_stash(path: &Path, stash_name: &str, stash_root: &Path) -> Result<()> {
    use crate::corpus::paths;

    if !path.exists() {
        return Err(anyhow::anyhow!(
            "Source file does not exist: {}",
            path.display()
        ));
    }

    let resolver = paths::get_resolver();

    // Convert absolute path to root-relative (includes zone dir, e.g., "corpus/Artist/Album/track.flac").
    // This is NOT a DB path — it's an intermediate for stripping the zone dir to get structure to preserve.
    let root_relative = resolver
        .to_relative(path)
        .ok_or_else(|| anyhow::anyhow!("Path not within archive root: {}", path.display()))?;

    // Strip the zone directory to get the preservable structure
    let preserved_path = root_relative
        .strip_prefix("corpus")
        .or_else(|_| root_relative.strip_prefix("libraries"))
        .unwrap_or(&root_relative);

    // Build destination: stash_root/stash_name/preserved_path
    let stash_base = stash_root.join(stash_name);
    let preferred = stash_base.join(preserved_path);

    // Create destination directory if needed
    if let Some(parent) = preferred.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create stash directory: {}", parent.display()))?;
    }

    let dest = match resolve_stash_destination(path, preferred)? {
        Some(dest) => dest,
        // Source already occupies its own stash slot — nothing to vacate.
        None => return Ok(()),
    };

    move_file(path, &dest).with_context(|| {
        format!(
            "Failed to move {} to stash at {}",
            path.display(),
            dest.display()
        )
    })?;

    Ok(())
}

/// Move a file, falling back to copy+delete if rename fails with EXDEV (cross-device).
fn move_file(src: &Path, dest: &Path) -> Result<()> {
    // EXDEV = 18 on Linux (cross-device link)
    const EXDEV: i32 = 18;

    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(EXDEV) => {
            // Cross-device move: copy then delete
            crate::logging::log_general(format!(
                "[WARNING] Cross-device move fallback (copy+delete): {} -> {}",
                src.display(),
                dest.display()
            ));
            fs::copy(src, dest).with_context(|| {
                format!("Failed to copy {} to {}", src.display(), dest.display())
            })?;
            fs::remove_file(src).with_context(|| {
                format!("Failed to remove source after copy: {}", src.display())
            })?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_move_mutation_structure() {
        let mutation = Mutation::Move(MoveMutation {
            source: PathBuf::from("/src/file.flac"),
            destination: PathBuf::from("/dst/file.flac"),
        });

        assert!(matches!(mutation, Mutation::Move(_)));
    }

    #[test]
    fn test_hard_link_mutation_structure() {
        let mutation = Mutation::HardLink(HardLinkMutation {
            source: PathBuf::from("/corpus/file.flac"),
            destination: PathBuf::from("/library/file.flac"),
        });

        assert!(matches!(mutation, Mutation::HardLink(_)));
    }

    /// A free stash slot is used as-is.
    #[test]
    fn stash_destination_uses_free_slot() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.flac");
        fs::write(&source, b"audio").unwrap();
        let preferred = dir.path().join("stash").join("track.flac");
        fs::create_dir_all(preferred.parent().unwrap()).unwrap();

        let dest = resolve_stash_destination(&source, preferred.clone()).unwrap();

        assert_eq!(dest, Some(preferred));
    }

    /// An occupied slot holding a *different* file uniquifies with underscores.
    #[test]
    fn stash_destination_uniquifies_past_unrelated_file() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.flac");
        fs::write(&source, b"audio").unwrap();
        let preferred = dir.path().join("stash").join("track.flac");
        fs::create_dir_all(preferred.parent().unwrap()).unwrap();
        fs::write(&preferred, b"someone else").unwrap();

        let dest = resolve_stash_destination(&source, preferred).unwrap();

        assert_eq!(dest, Some(dir.path().join("stash").join("track_2.flac")));
    }

    /// Regression: a stash entry hardlinked to the source must NOT short-circuit to
    /// a no-op. The caller stashes to vacate the source path; returning the existing
    /// entry (or None) leaves the source link in place and deadlocks the deploy
    /// pipeline on "Destination already exists (different inode)".
    #[test]
    fn stash_destination_skips_past_same_inode_entry() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.flac");
        fs::write(&source, b"audio").unwrap();
        let stash = dir.path().join("stash");
        fs::create_dir_all(&stash).unwrap();
        let preferred = stash.join("track.flac");
        // Same inode as source — the file's data is already stashed.
        fs::hard_link(&source, &preferred).unwrap();

        let dest = resolve_stash_destination(&source, preferred).unwrap();

        assert_eq!(dest, Some(stash.join("track_2.flac")));
        assert!(
            !dest.unwrap().exists(),
            "chosen destination must be a free path"
        );
    }

    /// Repeated same-inode entries keep uniquifying rather than giving up.
    #[test]
    fn stash_destination_skips_past_multiple_same_inode_entries() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.flac");
        fs::write(&source, b"audio").unwrap();
        let stash = dir.path().join("stash");
        fs::create_dir_all(&stash).unwrap();
        let preferred = stash.join("track.flac");
        fs::hard_link(&source, &preferred).unwrap();
        fs::hard_link(&source, stash.join("track_2.flac")).unwrap();
        fs::hard_link(&source, stash.join("track_3.flac")).unwrap();

        let dest = resolve_stash_destination(&source, preferred).unwrap();

        assert_eq!(dest, Some(stash.join("track_4.flac")));
    }

    /// Regression: the counting suffix keeps filenames short where the old unary
    /// scheme grew a byte per collision and eventually wedged the slot at NAME_MAX.
    /// This is the shape found in prod — a cover that had flapped 247 times.
    #[test]
    fn stash_destination_survives_hundreds_of_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cover.jpg");
        fs::write(&source, b"image").unwrap();
        let stash = dir.path().join("stash");
        fs::create_dir_all(&stash).unwrap();
        let preferred = stash.join("cover.jpg");
        fs::hard_link(&source, &preferred).unwrap();
        for n in 2..250 {
            fs::hard_link(&source, stash.join(format!("cover_{}.jpg", n))).unwrap();
        }

        let dest = resolve_stash_destination(&source, preferred).unwrap().unwrap();

        assert_eq!(dest, stash.join("cover_250.jpg"));
        assert!(
            dest.file_name().unwrap().len() < 20,
            "filename must not grow with collision count: {:?}",
            dest.file_name()
        );
    }

    /// A pathologically contended slot fails loudly instead of stat'ing forever.
    #[test]
    fn stash_destination_errors_past_attempt_cap() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cover.jpg");
        fs::write(&source, b"image").unwrap();
        let stash = dir.path().join("stash");
        fs::create_dir_all(&stash).unwrap();
        let preferred = stash.join("cover.jpg");
        fs::hard_link(&source, &preferred).unwrap();
        for n in 2..=STASH_UNIQUIFY_MAX_ATTEMPTS {
            fs::hard_link(&source, stash.join(format!("cover_{}.jpg", n))).unwrap();
        }

        let err = resolve_stash_destination(&source, preferred).unwrap_err();

        assert!(
            err.to_string().contains("flapping"),
            "unexpected error: {}",
            err
        );
    }

    /// A source that already sits at its own stash slot has nothing to vacate.
    #[test]
    fn stash_destination_noop_when_source_is_the_slot() {
        let dir = tempfile::tempdir().unwrap();
        let stash = dir.path().join("stash");
        fs::create_dir_all(&stash).unwrap();
        let source = stash.join("track.flac");
        fs::write(&source, b"audio").unwrap();

        let dest = resolve_stash_destination(&source, source.clone()).unwrap();

        assert_eq!(dest, None);
    }

    /// The underscore loop is bounded by NAME_MAX instead of spinning forever.
    #[test]
    fn stash_destination_errors_past_name_max() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.flac");
        fs::write(&source, b"audio").unwrap();
        let stash = dir.path().join("stash");
        fs::create_dir_all(&stash).unwrap();

        // Stem already at NAME_MAX minus the extension: the first suffix overflows.
        let stem = "x".repeat(STASH_FILENAME_MAX_BYTES - ".flac".len());
        let preferred = stash.join(format!("{}.flac", stem));
        fs::write(&preferred, b"someone else").unwrap();

        let err = resolve_stash_destination(&source, preferred).unwrap_err();

        assert!(
            err.to_string().contains("NAME_MAX"),
            "unexpected error: {}",
            err
        );
    }
}
