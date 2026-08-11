//! Applying integrations to the parent worktree (v1.4 §PR7, §PR12).
//!
//! The decision logic lives in `runtime-core` where it is pure and testable.
//! What lives here is the part that touches git: reading a worker's patch,
//! comparing pending proposals for conflicts, and applying an approved patch to
//! the parent worktree.
//!
//! One rule governs the whole file: **the caller must already hold an approval**
//! for the exact digest being applied. [`apply_to_parent`] re-derives the digest
//! of the bytes it is about to apply and refuses if it differs, so an approval
//! for one patch can never be spent on another.

use crate::DelegationRuntimeError;
use crate::workspace::WorkerPatch;
use purrcode_repository_engine::{RepositoryEngine, SessionWorktree};
use purrcode_runtime_core::delegation::DelegationId;
use purrcode_runtime_core::delegation::integration::{
    IntegrationConflict, PatchHunk, detect_conflicts, parse_unified_diff,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One pending proposal, in the form conflict detection needs.
#[derive(Clone, Debug)]
pub struct PendingPatch {
    pub delegation_id: DelegationId,
    pub changed_paths: Vec<PathBuf>,
    pub patch: Vec<u8>,
}

impl PendingPatch {
    pub fn from_worker_patch(delegation_id: DelegationId, patch: &WorkerPatch) -> Self {
        Self {
            delegation_id,
            changed_paths: patch.changed_paths.clone(),
            patch: patch.patch.clone(),
        }
    }
}

/// Compare every pending proposal against every other and report conflicts
/// (v1.4 §PR7 "Conflict Rules").
///
/// A changed path with no parsable hunk — a binary file, or a diff shape this
/// parser does not understand — is reported to `detect_conflicts` as *opaque*,
/// which treats two workers touching it as a hard conflict. Guessing the other
/// way would auto-merge an asset two workers both rewrote.
pub fn conflicts_among(pending: &[PendingPatch]) -> Vec<IntegrationConflict> {
    let mut hunks: BTreeMap<DelegationId, Vec<PatchHunk>> = BTreeMap::new();
    let mut opaque: BTreeMap<DelegationId, Vec<PathBuf>> = BTreeMap::new();
    for patch in pending {
        let parsed = parse_unified_diff(&patch.patch);
        let described: std::collections::BTreeSet<&PathBuf> =
            parsed.iter().map(|hunk| &hunk.path).collect();
        let unparsed: Vec<PathBuf> = patch
            .changed_paths
            .iter()
            .filter(|path| !described.contains(path))
            .cloned()
            .collect();
        if !parsed.is_empty() {
            hunks.insert(patch.delegation_id, parsed);
        }
        if !unparsed.is_empty() {
            opaque.insert(patch.delegation_id, unparsed);
        }
    }
    detect_conflicts(&hunks, &opaque)
}

/// The hunks one patch contains, for a single proposal.
pub fn hunks_of(patch: &[u8]) -> Vec<PatchHunk> {
    parse_unified_diff(patch)
}

/// Apply an approved patch to the parent worktree.
///
/// `approved_digest` is what a human (or policy) actually approved. The bytes
/// are hashed again here and compared: an approval is for a specific patch, not
/// for "whatever the worker has now".
pub async fn apply_to_parent(
    parent: &SessionWorktree,
    patch: &[u8],
    approved_digest: &str,
) -> Result<AppliedIntegration, DelegationRuntimeError> {
    let digest = blake3::hash(patch).to_hex().to_string();
    if digest != approved_digest {
        return Err(DelegationRuntimeError::PatchDigestMismatch {
            approved: approved_digest.to_owned(),
            actual: digest,
        });
    }
    if patch.is_empty() {
        return Err(DelegationRuntimeError::EmptyPatch);
    }
    let before = RepositoryEngine::effects(parent).await?;
    // `apply_patch` runs `git apply --check` first, so a patch that would
    // conflict textually fails before anything is written.
    RepositoryEngine::apply_patch(parent, patch).await?;
    let after = RepositoryEngine::effects(parent).await?;
    let newly_changed: Vec<PathBuf> = after
        .changed_files
        .iter()
        .filter(|path| !before.changed_files.contains(path))
        .cloned()
        .collect();
    Ok(AppliedIntegration {
        patch_digest: digest,
        changed_paths: after.changed_files,
        newly_changed_paths: newly_changed,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedIntegration {
    pub patch_digest: String,
    /// Everything the parent worktree now has changed.
    pub changed_paths: Vec<PathBuf>,
    /// Paths this integration introduced, as opposed to ones the parent had
    /// already touched.
    pub newly_changed_paths: Vec<PathBuf>,
}

/// Build the patch for a subset of a worker's hunks (v1.4 §PR12).
///
/// The result is a *different* patch, so it gets a different digest — reusing
/// the worker's would make evidence claim the reviewed bytes and the applied
/// bytes were the same.
///
/// Selection is by hunk index within the parsed diff, matching what the review
/// UI displays. An index that does not exist is an error rather than a silent
/// omission: quietly applying fewer hunks than the user selected is the same
/// class of bug as applying more.
pub fn select_hunks(patch: &[u8], selected: &[usize]) -> Result<Vec<u8>, DelegationRuntimeError> {
    let text = String::from_utf8(patch.to_vec())
        .map_err(|_| DelegationRuntimeError::BinaryHunkSelection)?;
    let lines: Vec<&str> = text.lines().collect();

    // Split into (file header lines, [hunk line ranges]).
    let mut out = String::new();
    let mut hunk_index = 0_usize;
    let mut pending_header: Vec<&str> = Vec::new();
    let mut header_written = false;
    let mut keeping = false;
    let mut kept = 0_usize;

    for line in lines {
        if line.starts_with("diff --git ") {
            pending_header = vec![line];
            header_written = false;
            keeping = false;
            continue;
        }
        if line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("index ")
            || line.starts_with("new file mode ")
            || line.starts_with("deleted file mode ")
            || line.starts_with("old mode ")
            || line.starts_with("new mode ")
            || line.starts_with("similarity index ")
            || line.starts_with("rename from ")
            || line.starts_with("rename to ")
        {
            pending_header.push(line);
            continue;
        }
        if line.starts_with("GIT binary patch") {
            return Err(DelegationRuntimeError::BinaryHunkSelection);
        }
        if line.starts_with("@@ ") {
            keeping = selected.contains(&hunk_index);
            hunk_index += 1;
            if keeping {
                if !header_written {
                    for header in &pending_header {
                        out.push_str(header);
                        out.push('\n');
                    }
                    header_written = true;
                }
                out.push_str(line);
                out.push('\n');
                kept += 1;
            }
            continue;
        }
        if keeping {
            out.push_str(line);
            out.push('\n');
        }
    }

    if let Some(missing) = selected.iter().find(|index| **index >= hunk_index) {
        return Err(DelegationRuntimeError::UnknownHunk { index: *missing });
    }
    if kept == 0 {
        return Err(DelegationRuntimeError::EmptyPatch);
    }
    Ok(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_FILES: &str = "\
diff --git a/src/auth/token.rs b/src/auth/token.rs
--- a/src/auth/token.rs
+++ b/src/auth/token.rs
@@ -10,3 +10,4 @@ fn exchange() {
     let a = 1;
+    let b = 2;
     ok()
diff --git a/src/auth/store.rs b/src/auth/store.rs
--- a/src/auth/store.rs
+++ b/src/auth/store.rs
@@ -1,2 +1,3 @@
 pub struct Store;
+impl Store {}
";

    fn patch(delegation: DelegationId, body: &str, paths: &[&str]) -> PendingPatch {
        PendingPatch {
            delegation_id: delegation,
            changed_paths: paths.iter().map(PathBuf::from).collect(),
            patch: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn disjoint_files_produce_no_conflicts() {
        let a = DelegationId::new();
        let b = DelegationId::new();
        let first = patch(
            a,
            "--- a/src/auth/token.rs\n+++ b/src/auth/token.rs\n@@ -10,3 +10,4 @@\n+x\n",
            &["src/auth/token.rs"],
        );
        let second = patch(
            b,
            "--- a/migrations/0007.sql\n+++ b/migrations/0007.sql\n@@ -1,2 +1,3 @@\n+y\n",
            &["migrations/0007.sql"],
        );
        assert!(conflicts_among(&[first, second]).is_empty());
    }

    #[test]
    fn overlapping_hunks_in_one_file_conflict() {
        let a = DelegationId::new();
        let b = DelegationId::new();
        let first = patch(
            a,
            "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -10,5 +10,6 @@\n+x\n",
            &["src/lib.rs"],
        );
        let second = patch(
            b,
            "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -12,5 +12,6 @@\n+y\n",
            &["src/lib.rs"],
        );
        let conflicts = conflicts_among(&[first, second]);
        assert_eq!(conflicts.len(), 1);
        assert!(!conflicts[0].kind.is_auto_mergeable());
    }

    #[test]
    fn a_binary_file_two_workers_touched_is_a_hard_conflict() {
        // Neither side parses into hunks, so both are opaque: the conservative
        // answer is a conflict, not an auto-merge.
        let a = DelegationId::new();
        let b = DelegationId::new();
        let first = patch(a, "GIT binary patch\nliteral 12\n", &["assets/logo.png"]);
        let second = patch(b, "GIT binary patch\nliteral 14\n", &["assets/logo.png"]);
        let conflicts = conflicts_among(&[first, second]);
        assert_eq!(conflicts.len(), 1);
        assert!(!conflicts[0].kind.is_auto_mergeable());
    }

    #[test]
    fn selecting_one_hunk_produces_a_smaller_patch_with_its_own_digest() {
        let full = TWO_FILES.as_bytes();
        let subset = select_hunks(full, &[1]).unwrap();
        let text = String::from_utf8(subset.clone()).unwrap();
        assert!(text.contains("src/auth/store.rs"));
        assert!(
            !text.contains("src/auth/token.rs"),
            "an unselected file must not appear: {text}"
        );
        assert_eq!(parse_unified_diff(&subset).len(), 1);
        assert_ne!(
            blake3::hash(&subset).to_hex().to_string(),
            blake3::hash(full).to_hex().to_string(),
            "an amended patch must not reuse the worker's digest"
        );
    }

    #[test]
    fn selecting_every_hunk_keeps_both_files() {
        let subset = select_hunks(TWO_FILES.as_bytes(), &[0, 1]).unwrap();
        assert_eq!(parse_unified_diff(&subset).len(), 2);
    }

    #[test]
    fn selecting_an_unknown_hunk_is_an_error_not_a_silent_omission() {
        assert!(matches!(
            select_hunks(TWO_FILES.as_bytes(), &[9]),
            Err(DelegationRuntimeError::UnknownHunk { index: 9 })
        ));
        assert!(matches!(
            select_hunks(TWO_FILES.as_bytes(), &[]),
            Err(DelegationRuntimeError::EmptyPatch)
        ));
    }

    #[test]
    fn binary_patches_cannot_be_hunk_selected() {
        let binary = "diff --git a/logo.png b/logo.png\nGIT binary patch\nliteral 4\n";
        assert!(matches!(
            select_hunks(binary.as_bytes(), &[0]),
            Err(DelegationRuntimeError::BinaryHunkSelection)
        ));
    }
}
