//! Review state: changed files, hunks, and bounded navigation.
//!
//! The changed-file list comes from the daemon's porcelain evidence and the
//! hunks come from the same patch the daemon produced. Nothing here re-derives
//! what changed from the filesystem, and there is no second diff source.
//!
//! Large diffs stay bounded: hunk bodies are sliced on demand from the patch
//! text rather than materialized for every file up front, so opening review on a
//! ten-thousand-line patch costs one index pass.

use crate::activity::{effects_from_porcelain, EffectStatus, FileEffect};

/// One hunk, stored as a range into the patch text rather than a copy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hunk {
    pub header: String,
    /// Line index of the hunk header within the patch.
    pub start_line: usize,
    /// Number of lines in the hunk body, excluding the header.
    pub line_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewFile {
    pub path: String,
    pub status: EffectStatus,
    pub hunks: Vec<Hunk>,
    /// True when the patch reports a binary difference, which has no reviewable
    /// text body.
    pub binary: bool,
}

impl ReviewFile {
    pub fn summary(&self) -> String {
        self.summary_with(" · ")
    }

    /// `separator` comes from the active symbol set so the line stays renderable
    /// on a terminal without Unicode.
    pub fn summary_with(&self, separator: &str) -> String {
        if self.binary {
            return format!("{}{separator}binary", self.status.word());
        }
        format!(
            "{}{separator}{} hunk(s)",
            self.status.word(),
            self.hunks.len()
        )
    }
}

/// How many patch lines the review renders for one hunk before truncating.
const MAX_HUNK_LINES: usize = 400;

#[derive(Clone, Debug, Default)]
pub struct ReviewState {
    /// The patch text as the daemon produced it, split into lines once.
    patch_lines: Vec<String>,
    pub files: Vec<ReviewFile>,
    pub selected_file: usize,
    pub selected_hunk: usize,
    /// Files reported as changed by porcelain but absent from the patch, e.g.
    /// untracked files with no diff yet. Surfaced rather than dropped.
    pub without_patch: Vec<FileEffect>,
}

impl ReviewState {
    /// Build review state from the daemon's diff response.
    pub fn from_daemon(patch: &str, porcelain: &str) -> Self {
        let patch_lines: Vec<String> = patch.lines().map(str::to_owned).collect();
        let effects = effects_from_porcelain(porcelain);
        let mut files = parse_patch(&patch_lines);

        // Porcelain is the authority on status; the patch is the authority on
        // hunks. Reconcile so a file never shows a status the repository engine
        // did not report.
        for file in &mut files {
            if let Some(effect) = effects.iter().find(|effect| effect.path == file.path) {
                file.status = effect.status;
            }
        }
        let without_patch = effects
            .iter()
            .filter(|effect| !files.iter().any(|file| file.path == effect.path))
            .cloned()
            .collect();

        Self {
            patch_lines,
            files,
            selected_file: 0,
            selected_hunk: 0,
            without_patch,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.without_patch.is_empty()
    }

    pub fn file(&self) -> Option<&ReviewFile> {
        self.files.get(self.selected_file)
    }

    pub fn total_hunks(&self) -> usize {
        self.files.iter().map(|file| file.hunks.len()).sum()
    }

    /// Lines of the selected hunk, sliced from the patch on demand and bounded.
    /// Returns `(lines, truncated_count)`.
    pub fn selected_hunk_lines(&self) -> (Vec<&str>, usize) {
        let Some(file) = self.file() else {
            return (Vec::new(), 0);
        };
        let Some(hunk) = file.hunks.get(self.selected_hunk) else {
            return (Vec::new(), 0);
        };
        let start = hunk.start_line;
        let end = (start + hunk.line_count + 1).min(self.patch_lines.len());
        let all = &self.patch_lines[start..end];
        if all.len() > MAX_HUNK_LINES {
            (
                all[..MAX_HUNK_LINES].iter().map(String::as_str).collect(),
                all.len() - MAX_HUNK_LINES,
            )
        } else {
            (all.iter().map(String::as_str).collect(), 0)
        }
    }

    pub fn next_file(&mut self) {
        if self.files.is_empty() {
            return;
        }
        self.selected_file = (self.selected_file + 1) % self.files.len();
        self.selected_hunk = 0;
    }

    pub fn previous_file(&mut self) {
        if self.files.is_empty() {
            return;
        }
        self.selected_file = if self.selected_file == 0 {
            self.files.len() - 1
        } else {
            self.selected_file - 1
        };
        self.selected_hunk = 0;
    }

    /// Move to the next hunk, rolling into the next file at the end of one.
    pub fn next_hunk(&mut self) {
        let Some(file) = self.file() else { return };
        if self.selected_hunk + 1 < file.hunks.len() {
            self.selected_hunk += 1;
        } else {
            self.next_file();
        }
    }

    pub fn previous_hunk(&mut self) {
        if self.selected_hunk > 0 {
            self.selected_hunk -= 1;
            return;
        }
        self.previous_file();
        if let Some(file) = self.file() {
            self.selected_hunk = file.hunks.len().saturating_sub(1);
        }
    }

    pub fn summary(&self) -> String {
        self.summary_with(" · ")
    }

    pub fn summary_with(&self, separator: &str) -> String {
        let files = self.files.len() + self.without_patch.len();
        if files == 0 {
            return "no recorded changes".into();
        }
        format!("{files} file(s){separator}{} hunk(s)", self.total_hunks())
    }
}

/// Index a unified diff into per-file hunk ranges without copying hunk bodies.
fn parse_patch(lines: &[String]) -> Vec<ReviewFile> {
    let mut files: Vec<ReviewFile> = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let line = &lines[index];
        if let Some(path) = diff_header_path(line) {
            files.push(ReviewFile {
                path,
                // Overwritten from porcelain when available; a patch alone cannot
                // reliably distinguish added from modified.
                status: EffectStatus::Modified,
                hunks: Vec::new(),
                binary: false,
            });
            index += 1;
            continue;
        }
        if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            if let Some(file) = files.last_mut() {
                file.binary = true;
            }
            index += 1;
            continue;
        }
        if line.starts_with("@@") {
            let start_line = index;
            let mut body = 0usize;
            index += 1;
            while index < lines.len() {
                let next = &lines[index];
                if next.starts_with("@@") || diff_header_path(next).is_some() {
                    break;
                }
                body += 1;
                index += 1;
            }
            if let Some(file) = files.last_mut() {
                file.hunks.push(Hunk {
                    header: lines[start_line].clone(),
                    start_line,
                    line_count: body,
                });
            }
            continue;
        }
        index += 1;
    }
    files
}

/// Extract the post-image path from a `diff --git a/x b/y` header.
fn diff_header_path(line: &str) -> Option<String> {
    let remainder = line.strip_prefix("diff --git ")?;
    let (_, post) = remainder.rsplit_once(" b/")?;
    Some(post.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 111..222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 fn one() {}
+fn two() {}
 fn three() {}
@@ -20,2 +21,3 @@
 fn twenty() {}
+fn added() {}
diff --git a/src/new.rs b/src/new.rs
new file mode 100644
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1,2 @@
+fn brand() {}
+fn new() {}
diff --git a/logo.png b/logo.png
Binary files a/logo.png and b/logo.png differ
";

    const PORCELAIN: &str = " M src/lib.rs\nA  src/new.rs\nM  logo.png\n?? notes.txt\n";

    fn state() -> ReviewState {
        ReviewState::from_daemon(PATCH, PORCELAIN)
    }

    #[test]
    fn files_and_hunks_come_from_the_daemon_patch() {
        let review = state();
        let paths: Vec<&str> = review.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, vec!["src/lib.rs", "src/new.rs", "logo.png"]);
        assert_eq!(review.files[0].hunks.len(), 2);
        assert_eq!(review.files[1].hunks.len(), 1);
        assert_eq!(review.total_hunks(), 3);
    }

    #[test]
    fn status_comes_from_porcelain_not_from_the_patch() {
        let review = state();
        assert_eq!(review.files[0].status, EffectStatus::Modified);
        assert_eq!(
            review.files[1].status,
            EffectStatus::Added,
            "porcelain reported an addition; the patch alone could not"
        );
    }

    #[test]
    fn binary_effects_are_marked_and_have_no_reviewable_body() {
        let review = state();
        let binary = review
            .files
            .iter()
            .find(|file| file.path == "logo.png")
            .unwrap();
        assert!(binary.binary);
        assert!(binary.hunks.is_empty());
        assert!(binary.summary().contains("binary"));
    }

    #[test]
    fn a_changed_file_with_no_patch_body_is_surfaced_not_dropped() {
        let review = state();
        assert_eq!(
            review
                .without_patch
                .iter()
                .map(|effect| effect.path.as_str())
                .collect::<Vec<_>>(),
            vec!["notes.txt"],
            "an untracked file with no diff must still be reviewable"
        );
        assert!(review.summary().contains("4 file(s)"));
    }

    #[test]
    fn file_navigation_wraps_and_resets_the_hunk() {
        let mut review = state();
        review.selected_hunk = 1;
        review.next_file();
        assert_eq!(review.selected_file, 1);
        assert_eq!(
            review.selected_hunk, 0,
            "a new file starts at its first hunk"
        );
        review.next_file();
        review.next_file();
        assert_eq!(review.selected_file, 0, "file navigation wraps");
        review.previous_file();
        assert_eq!(review.selected_file, 2);
    }

    #[test]
    fn hunk_navigation_rolls_into_the_next_file() {
        let mut review = state();
        assert_eq!((review.selected_file, review.selected_hunk), (0, 0));
        review.next_hunk();
        assert_eq!((review.selected_file, review.selected_hunk), (0, 1));
        review.next_hunk();
        assert_eq!(
            (review.selected_file, review.selected_hunk),
            (1, 0),
            "the end of a file's hunks continues into the next file"
        );
    }

    #[test]
    fn previous_hunk_walks_back_into_the_previous_file() {
        let mut review = state();
        review.selected_file = 1;
        review.selected_hunk = 0;
        review.previous_hunk();
        assert_eq!(review.selected_file, 0);
        assert_eq!(
            review.selected_hunk, 1,
            "walking back lands on the previous file's last hunk"
        );
    }

    #[test]
    fn the_selected_hunk_body_is_sliced_from_the_patch() {
        let review = state();
        let (lines, truncated) = review.selected_hunk_lines();
        assert_eq!(truncated, 0);
        assert_eq!(lines[0], "@@ -1,3 +1,4 @@");
        assert!(lines.contains(&"+fn two() {}"));
        assert!(
            !lines.iter().any(|line| line.starts_with("@@ -20")),
            "one hunk must not bleed into the next: {lines:?}"
        );
    }

    #[test]
    fn a_large_hunk_is_bounded_and_reports_what_it_withheld() {
        let mut patch = String::from("diff --git a/big.rs b/big.rs\n@@ -1,5000 +1,5000 @@\n");
        for index in 0..5_000 {
            patch.push_str(&format!("+line {index}\n"));
        }
        let review = ReviewState::from_daemon(&patch, "M  big.rs\n");
        let (lines, truncated) = review.selected_hunk_lines();
        assert_eq!(lines.len(), MAX_HUNK_LINES);
        assert_eq!(truncated, 5_001 - MAX_HUNK_LINES);
    }

    #[test]
    fn indexing_a_large_patch_stays_responsive() {
        let mut patch = String::new();
        for file in 0..200 {
            patch.push_str(&format!("diff --git a/file{file}.rs b/file{file}.rs\n"));
            for hunk in 0..10 {
                patch.push_str(&format!("@@ -{hunk},5 +{hunk},6 @@\n"));
                for line in 0..20 {
                    patch.push_str(&format!(" context {line}\n"));
                }
            }
        }
        let started = std::time::Instant::now();
        let review = ReviewState::from_daemon(&patch, "");
        assert_eq!(review.files.len(), 200);
        assert_eq!(review.total_hunks(), 2_000);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(250),
            "indexing a large patch must stay interactive"
        );
    }

    #[test]
    fn an_empty_diff_is_empty_not_a_panic() {
        let review = ReviewState::from_daemon("", "");
        assert!(review.is_empty());
        assert_eq!(review.summary(), "no recorded changes");
        assert_eq!(review.selected_hunk_lines(), (Vec::new(), 0));
        let mut review = review;
        review.next_file();
        review.previous_file();
        review.next_hunk();
        review.previous_hunk();
        assert_eq!(review.selected_file, 0);
    }

    #[test]
    fn a_patch_with_no_hunks_still_lists_the_file() {
        let review = ReviewState::from_daemon(
            "diff --git a/empty.rs b/empty.rs\nold mode 100644\nnew mode 100755\n",
            "M  empty.rs\n",
        );
        assert_eq!(review.files.len(), 1);
        assert!(review.files[0].hunks.is_empty());
        assert!(review.files[0].summary().contains("0 hunk(s)"));
    }

    #[test]
    fn paths_containing_b_slash_are_parsed_correctly() {
        let review = ReviewState::from_daemon(
            "diff --git a/src/b/mod.rs b/src/b/mod.rs\n@@ -1 +1 @@\n-x\n+y\n",
            "M  src/b/mod.rs\n",
        );
        assert_eq!(review.files[0].path, "src/b/mod.rs");
    }
}
