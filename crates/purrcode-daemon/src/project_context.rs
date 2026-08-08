//! Turns the project's standing knowledge and the user's composer references
//! into the [`PinnedContext`] a turn actually carries.
//!
//! This module is the answer to a specific class of dishonesty the v1.2 review
//! found: the IDE showed a resolved `@file` chip and a Project Memory settings
//! page, both of which described content that never reached the model. A chip
//! that says "attached" has to correspond to bytes in the prompt, and a memory
//! entry the product calls "remembered" has to be consumable by an ordinary
//! coding turn — not only by a management API.
//!
//! Three rules shape everything here:
//!
//! 1. **The daemon resolves, not the client.** References are re-resolved from
//!    the request text at admission rather than trusting content a client
//!    uploaded, so the attached bytes are the repository's, and every client
//!    (IDE, TUI, CLI) gets the same behaviour for free.
//! 2. **Bounded, and honest about it.** Every attachment has a byte budget and
//!    says so inline when it truncates. Silently attaching half a file is the
//!    same lie as attaching nothing.
//! 3. **Repository content is data.** Attachments are labelled as untrusted
//!    material so a `@file` whose contents say "ignore your instructions"
//!    stays a file the model is reading, not an instruction it is following.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use purrcode_ninelives::ProjectMemoryEntry;
use purrcode_reference_resolver::{Reference, resolve_refs};
use purrcode_runtime_core::{PinnedContext, PinnedOrigin, PinnedSection};

/// Per-attachment content budget. Large enough that a real source file arrives
/// whole, small enough that four references cannot evict the conversation.
const MAX_ATTACHMENT_BYTES: usize = 24 * 1024;
/// Total budget across every composer reference in one turn.
const MAX_REFERENCE_BYTES_TOTAL: usize = 96 * 1024;
/// Per-file budget for a project instruction file.
const MAX_INSTRUCTION_BYTES: usize = 16 * 1024;
/// Total budget for selected project memory.
const MAX_MEMORY_BYTES_TOTAL: usize = 12 * 1024;
/// Most memory entries pinned into one turn. Memory is meant to be a handful
/// of durable facts; a hundred of them is a database dump, and dumping the
/// database is what makes models ignore all of it.
const MAX_MEMORY_ENTRIES: usize = 16;
/// The instruction files a repository may use to address the agent, in the
/// order they are attached.
const INSTRUCTION_FILES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", ".purrcode.md"];

/// A reference the daemon tried to attach, and what happened. Returned
/// alongside the pinned context so the caller can record an audit trail: an
/// unresolved reference must be reported, never silently dropped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReferenceOutcome {
    pub display: String,
    pub attached: bool,
    pub detail: Option<String>,
}

/// What one assembly pass produced.
pub(crate) struct AssembledContext {
    pub pinned: PinnedContext,
    pub references: Vec<ReferenceOutcome>,
}

/// Build the pinned context for a turn.
///
/// `request_text` is the text the turn is actually responding to — the
/// objective for a new session, the follow-up message for a continuation.
/// References are parsed from it, so `@src/auth.rs` in a follow-up attaches
/// that file to *that* turn rather than to the session's original objective.
pub(crate) async fn assemble(
    repository: &Path,
    request_text: &str,
    memory: &[ProjectMemoryEntry],
) -> AssembledContext {
    let mut sections = Vec::new();
    let mut references = Vec::new();

    // Project instructions first: they are the standing rules the rest of the
    // turn is read against.
    for name in INSTRUCTION_FILES {
        if let Some(content) = read_bounded(&repository.join(name), MAX_INSTRUCTION_BYTES) {
            sections.push(PinnedSection {
                origin: PinnedOrigin::ProjectInstructions,
                label: name.to_owned(),
                content,
                memory_id: None,
            });
        }
    }

    for section in select_memory(memory, request_text) {
        sections.push(section);
    }

    let mut spent = 0usize;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for parsed in resolve_refs(request_text).unwrap_or_default() {
        let display = parsed.reference.display();
        // The same reference typed twice is attached once. Two copies of a file
        // is budget spent to tell the model nothing new.
        if !seen.insert(display.clone()) {
            continue;
        }
        if spent >= MAX_REFERENCE_BYTES_TOTAL {
            references.push(ReferenceOutcome {
                display,
                attached: false,
                detail: Some("not attached: this turn's reference budget was already full".into()),
            });
            continue;
        }
        let remaining = (MAX_REFERENCE_BYTES_TOTAL - spent).min(MAX_ATTACHMENT_BYTES);
        match attach_reference(repository, &parsed.reference, remaining).await {
            Ok(content) => {
                spent += content.len();
                references.push(ReferenceOutcome {
                    display: display.clone(),
                    attached: true,
                    detail: None,
                });
                sections.push(PinnedSection {
                    origin: PinnedOrigin::ComposerReference,
                    label: display,
                    content,
                    memory_id: None,
                });
            }
            Err(reason) => references.push(ReferenceOutcome {
                display,
                attached: false,
                detail: Some(reason),
            }),
        }
    }

    AssembledContext {
        pinned: PinnedContext { sections },
        references,
    }
}

/// How much of a reference the composer chip shows.
const MAX_PREVIEW_BYTES: usize = 600;

/// What the composer should show for one reference before the user sends it.
///
/// Deliberately the *same* resolution path [`assemble`] uses, only with a
/// smaller budget. That is the whole point: the chip's `resolved` flag then
/// means "this will be attached to the turn", which is what a user reads a
/// green chip as. Two code paths — one deciding what to display and one
/// deciding what to attach — is how `@context` came to show a resolved chip for
/// something that could never be attached.
///
/// Returns `(resolved, preview, diagnostics)`.
pub(crate) async fn preview_reference(
    repository: &Path,
    reference: &Reference,
) -> (bool, Option<String>, Option<String>) {
    match attach_reference(repository, reference, MAX_PREVIEW_BYTES).await {
        Ok(content) => (true, Some(content), None),
        Err(reason) => (false, None, Some(reason)),
    }
}

/// Choose which memory entries this turn should carry.
///
/// Standing rules (`user_rules`) and build knowledge (`build`) are always
/// eligible: a rule that only applies when the request happens to share a word
/// with it is not a rule. Architecture and learnings are ranked by overlap with
/// the request, then by confidence and recency, because they are reference
/// material and a turn that pins all of it pins nothing usefully.
fn select_memory(memory: &[ProjectMemoryEntry], request_text: &str) -> Vec<PinnedSection> {
    let terms = significant_terms(request_text);
    let mut scored: Vec<(u32, &ProjectMemoryEntry)> = memory
        .iter()
        .map(|entry| (memory_score(entry, &terms), entry))
        .filter(|(score, _)| *score > 0)
        .collect();
    // Descending score; ties broken by most-recently-used, then newest, so the
    // ordering is total and the selection is stable across identical turns.
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| right.last_used_at.cmp(&left.last_used_at))
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut sections = Vec::new();
    let mut spent = 0usize;
    for (_, entry) in scored.into_iter().take(MAX_MEMORY_ENTRIES) {
        if spent >= MAX_MEMORY_BYTES_TOTAL {
            break;
        }
        let budget = MAX_MEMORY_BYTES_TOTAL - spent;
        let mut content = truncate_on_char_boundary(&entry.content, budget);
        spent += content.len();
        // Same rule as a file attachment: a truncated entry must not look
        // complete. A half-quoted rule read as the whole rule is worse than a
        // rule that was left out.
        if content.len() < entry.content.len() {
            content.push_str("\n… [truncated: this memory entry is longer than the turn's budget]");
        }
        // Provenance travels with the content. A model that is told a fact is
        // "unverified, from a session summary" can weigh it against what it
        // reads in the repository; a bare assertion it cannot source is
        // indistinguishable from ground truth.
        sections.push(PinnedSection {
            origin: PinnedOrigin::ProjectMemory,
            label: format!("{} ({}, {})", entry.kind, entry.confidence, entry.scope),
            content: format!("{content}\n(source: {})", entry.source),
            memory_id: Some(entry.id.to_string()),
        });
    }
    sections
}

/// Relevance score for one memory entry, or 0 to leave it out.
fn memory_score(entry: &ProjectMemoryEntry, terms: &BTreeSet<String>) -> u32 {
    let base = match entry.kind.as_str() {
        // Always carried: these are instructions and project facts that do not
        // become irrelevant just because the request did not name them.
        "user_rules" => 100,
        "build" => 80,
        // Carried when the request touches them.
        _ => 0,
    };
    let overlap = {
        let content_terms = significant_terms(&entry.content);
        terms.intersection(&content_terms).count().min(10) as u32 * 4
    };
    let confidence = match entry.confidence.as_str() {
        "verified" => 6,
        "likely" => 3,
        _ => 0,
    };
    let scope = match entry.scope.as_str() {
        "repository" => 2,
        _ => 0,
    };
    let score = base + overlap + confidence + scope;
    // An entry with neither a standing-rule kind nor any overlap with the
    // request is not pinned. Confidence alone is not relevance.
    if base == 0 && overlap == 0 { 0 } else { score }
}

/// The words in a text worth matching on: lowercase, de-punctuated, longer
/// than three characters, and not one of the words every request contains.
fn significant_terms(text: &str) -> BTreeSet<String> {
    const STOP_WORDS: [&str; 24] = [
        "this", "that", "with", "from", "into", "have", "will", "should", "would", "could", "what",
        "when", "where", "which", "there", "then", "than", "here", "make", "does", "just", "like",
        "also", "please",
    ];
    text.split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|word| word.len() > 3)
        .map(str::to_ascii_lowercase)
        .filter(|word| !STOP_WORDS.contains(&word.as_str()))
        .collect()
}

/// Resolve one reference to the bounded content that will be attached.
///
/// Returns `Err` with a reason the caller can show the user. Failing loudly is
/// the point: a reference the daemon could not attach must not look attached.
async fn attach_reference(
    repository: &Path,
    reference: &Reference,
    budget: usize,
) -> Result<String, String> {
    match reference {
        Reference::File { path, range } => {
            let absolute = confine(repository, path)?;
            if !absolute.is_file() {
                return Err(format!("`{path}` is not a file in this repository"));
            }
            let content = read_bounded(&absolute, budget)
                .ok_or_else(|| format!("`{path}` could not be read as text"))?;
            Ok(match range {
                Some((start, end)) => {
                    let lines: Vec<&str> = content.lines().collect();
                    let first = (*start as usize).saturating_sub(1);
                    let last = (*end as usize).min(lines.len());
                    if first >= last {
                        return Err(format!(
                            "lines {start}-{end} are outside `{path}` ({} lines)",
                            lines.len()
                        ));
                    }
                    format!(
                        "UNTRUSTED FILE CONTENT — {path} lines {start}-{}:\n{}",
                        last,
                        lines[first..last].join("\n")
                    )
                }
                None => format!("UNTRUSTED FILE CONTENT — {path}:\n{content}"),
            })
        }
        Reference::Folder { path } => {
            let absolute = confine(repository, path)?;
            if !absolute.is_dir() {
                return Err(format!("`{path}` is not a folder in this repository"));
            }
            let listing = directory_listing(&absolute, budget);
            Ok(format!("UNTRUSTED DIRECTORY LISTING — {path}:\n{listing}"))
        }
        Reference::Diff => {
            let diff = git_text(repository, &["diff"], budget)
                .await
                .ok_or_else(|| "the working-tree diff could not be read".to_owned())?;
            if diff.trim().is_empty() {
                return Err("the working tree has no uncommitted changes".into());
            }
            Ok(format!("UNTRUSTED WORKING-TREE DIFF:\n{diff}"))
        }
        Reference::Git { reference } => {
            let text = git_text(repository, &["show", reference], budget)
                .await
                .ok_or_else(|| format!("`{reference}` is not a revision in this repository"))?;
            Ok(format!("UNTRUSTED GIT REVISION — {reference}:\n{text}"))
        }
        Reference::Symbol { name } => {
            // `--untracked` matters: in an agent IDE the file a user is asking
            // about is very often one that was just created and never staged,
            // and a plain `git grep` reports it as "not found".
            let hits = git_text(
                repository,
                &[
                    "grep",
                    "-n",
                    "--heading",
                    "--untracked",
                    "-e",
                    name.as_str(),
                ],
                budget,
            )
            .await
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| format!("`{name}` was not found in this repository's sources"))?;
            Ok(format!("UNTRUSTED SYMBOL OCCURRENCES — {name}:\n{hits}"))
        }
        // `@context` asks for the session's own assembled context, which the
        // turn already carries in full. Attaching a copy of the context to the
        // context is not a thing the runtime can honestly do, so it is
        // reported rather than faked.
        Reference::Context => Err(
            "@context describes this turn's own context, which is already assembled for the model"
                .into(),
        ),
    }
}

/// Join `relative` onto `repository`, refusing anything that could leave it.
///
/// Only plain path components are accepted: no `..`, no absolute prefix, no
/// root. The result is additionally required to stay under the repository once
/// symlinks are resolved, so a symlinked file inside the tree cannot be used
/// to read `/etc/shadow`.
fn confine(repository: &Path, relative: &str) -> Result<PathBuf, String> {
    let mut safe = PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(value) => safe.push(value),
            _ => return Err(format!("`{relative}` escapes the repository")),
        }
    }
    let joined = repository.join(&safe);
    // A path that does not exist yet cannot be canonicalized; report it as
    // missing rather than as an escape attempt.
    let canonical = joined
        .canonicalize()
        .map_err(|_| format!("`{relative}` was not found in this repository"))?;
    let root = repository
        .canonicalize()
        .map_err(|_| "the repository path could not be resolved".to_owned())?;
    if !canonical.starts_with(&root) {
        return Err(format!("`{relative}` escapes the repository"));
    }
    Ok(canonical)
}

/// Read up to `budget` bytes of a text file, appending an explicit truncation
/// note when the file is longer. Returns `None` for a file that is missing or
/// is not valid text.
fn read_bounded(path: &Path, budget: usize) -> Option<String> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    // One byte past the budget, so a file exactly at the budget is not
    // reported as truncated.
    file.by_ref()
        .take(budget as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let truncated = bytes.len() > budget;
    if truncated {
        bytes.truncate(budget);
    }
    let mut text = String::from_utf8(bytes).ok()?;
    if truncated {
        // Never leave a truncated attachment looking complete: the model must
        // know the file continues, or it will reason about a partial file as if
        // it were the whole one.
        text = truncate_on_char_boundary(&text, budget);
        text.push_str("\n… [truncated: attachment budget reached; read the file for the rest]");
    }
    Some(text)
}

/// A single-level listing of `directory`, bounded by `budget`.
fn directory_listing(directory: &Path, budget: usize) -> String {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return "(unreadable)".into();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    names.sort();
    let mut out = String::new();
    let mut dropped = 0usize;
    for name in names {
        if out.len() + name.len() + 1 > budget {
            dropped += 1;
            continue;
        }
        out.push_str(&name);
        out.push('\n');
    }
    if dropped > 0 {
        out.push_str(&format!("… [{dropped} more entries not listed]\n"));
    }
    out
}

/// Run a read-only git command in `repository` and return its bounded stdout.
async fn git_text(repository: &Path, args: &[&str], budget: usize) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .current_dir(repository)
        .args(args)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if text.len() <= budget {
        return Some(text);
    }
    let mut bounded = truncate_on_char_boundary(&text, budget);
    bounded.push_str("\n… [truncated: attachment budget reached]");
    Some(bounded)
}

/// Truncate to at most `budget` bytes without splitting a UTF-8 character.
fn truncate_on_char_boundary(text: &str, budget: usize) -> String {
    if text.len() <= budget {
        return text.to_owned();
    }
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn entry(kind: &str, content: &str) -> ProjectMemoryEntry {
        ProjectMemoryEntry {
            id: Uuid::new_v4(),
            repository: PathBuf::from("/repo"),
            kind: kind.into(),
            content: content.into(),
            source: "test".into(),
            confidence: "verified".into(),
            scope: "repository".into(),
            created_at: Utc::now(),
            last_used_at: None,
        }
    }

    #[test]
    fn standing_rules_are_pinned_even_when_the_request_does_not_mention_them() {
        let memory = vec![entry("user_rules", "Never commit directly to main.")];
        let selected = select_memory(&memory, "add a retry to the upload path");
        assert_eq!(selected.len(), 1, "a standing rule applies to every turn");
        assert!(
            selected[0]
                .content
                .contains("Never commit directly to main.")
        );
    }

    #[test]
    fn reference_material_is_pinned_only_when_the_request_touches_it() {
        let memory = vec![
            entry("architecture", "The scheduler owns retry backoff."),
            entry("learnings", "Terraform state lives in a separate bucket."),
        ];
        let selected = select_memory(&memory, "fix the scheduler backoff");
        let labels: Vec<&str> = selected
            .iter()
            .map(|section| section.label.as_str())
            .collect();
        assert!(
            labels.iter().any(|label| label.starts_with("architecture")),
            "the entry sharing 'scheduler'/'backoff' with the request must be pinned: {labels:?}"
        );
        assert!(
            !labels.iter().any(|label| label.starts_with("learnings")),
            "an unrelated entry must not consume the turn's budget: {labels:?}"
        );
    }

    #[test]
    fn every_pinned_memory_entry_carries_an_id_so_use_can_be_recorded() {
        let memory = vec![entry("build", "cargo test --workspace")];
        let selected = select_memory(&memory, "run the tests");
        assert!(
            selected.iter().all(|section| section.memory_id.is_some()),
            "without an id the daemon cannot record that memory was actually used"
        );
    }

    #[test]
    fn memory_selection_is_bounded() {
        let memory: Vec<ProjectMemoryEntry> = (0..64)
            .map(|index| entry("user_rules", &format!("rule {index}")))
            .collect();
        let selected = select_memory(&memory, "anything");
        assert!(
            selected.len() <= MAX_MEMORY_ENTRIES,
            "memory must not be dumped into the prompt wholesale"
        );
    }

    #[test]
    fn truncation_is_declared_rather_than_silent() {
        let directory = tempdir();
        let path = directory.join("long.txt");
        std::fs::write(&path, "x".repeat(4096)).unwrap();
        let content = read_bounded(&path, 128).expect("a text file reads");
        assert!(content.contains("truncated"), "{content}");
        assert!(content.len() < 4096);
    }

    #[test]
    fn a_file_within_budget_is_not_marked_truncated() {
        let directory = tempdir();
        let path = directory.join("short.txt");
        std::fs::write(&path, "fn main() {}").unwrap();
        let content = read_bounded(&path, 128).expect("a text file reads");
        assert_eq!(content, "fn main() {}");
    }

    #[test]
    fn paths_cannot_escape_the_repository() {
        let directory = tempdir();
        assert!(confine(&directory, "../etc/passwd").is_err());
        assert!(confine(&directory, "/etc/passwd").is_err());
    }

    #[tokio::test]
    async fn an_unresolvable_reference_is_reported_and_not_attached() {
        let directory = tempdir();
        let assembled = assemble(&directory, "please read @src/missing.rs", &[]).await;
        assert!(
            assembled
                .pinned
                .sections
                .iter()
                .all(|section| section.origin != PinnedOrigin::ComposerReference),
            "a missing file must not appear as attached content"
        );
        let outcome = assembled
            .references
            .iter()
            .find(|outcome| outcome.display == "@src/missing.rs")
            .expect("the attempt is reported");
        assert!(!outcome.attached);
        assert!(outcome.detail.is_some(), "the user is told why");
    }

    #[tokio::test]
    async fn a_resolvable_reference_is_attached_as_untrusted_content() {
        let directory = tempdir();
        std::fs::create_dir_all(directory.join("src")).unwrap();
        std::fs::write(directory.join("src/auth.rs"), "fn authenticate() {}").unwrap();
        let assembled = assemble(&directory, "fix this using @src/auth.rs", &[]).await;
        let attached = assembled
            .pinned
            .sections
            .iter()
            .find(|section| section.origin == PinnedOrigin::ComposerReference)
            .expect("the file is attached");
        assert_eq!(attached.label, "@src/auth.rs");
        assert!(attached.content.contains("fn authenticate() {}"));
        assert!(
            attached.content.contains("UNTRUSTED"),
            "attached repository content must be framed as data, not instructions"
        );
        assert!(
            assembled
                .references
                .iter()
                .any(|outcome| outcome.display == "@src/auth.rs" && outcome.attached)
        );
    }

    #[tokio::test]
    async fn a_symbol_in_a_brand_new_unstaged_file_still_resolves() {
        // The common case in an agent IDE: the file was just written and never
        // staged. A plain `git grep` reports it as absent, which would tell the
        // user their symbol does not exist.
        let directory = tempdir();
        assert!(
            std::process::Command::new("git")
                .current_dir(&directory)
                .args(["init", "--quiet"])
                .status()
                .is_ok_and(|status| status.success())
        );
        std::fs::write(directory.join("auth.rs"), "pub struct AuthMiddleware;\n").unwrap();
        let assembled = assemble(&directory, "explain #AuthMiddleware", &[]).await;
        let outcome = assembled
            .references
            .iter()
            .find(|outcome| outcome.display == "#AuthMiddleware")
            .expect("the symbol reference is attempted");
        assert!(
            outcome.attached,
            "an unstaged file's symbols must still resolve: {:?}",
            outcome.detail
        );
        let attached = assembled
            .pinned
            .sections
            .iter()
            .find(|section| section.origin == PinnedOrigin::ComposerReference)
            .expect("the occurrences are attached");
        assert!(attached.content.contains("AuthMiddleware"));
    }

    #[tokio::test]
    async fn project_instructions_are_attached_when_present() {
        let directory = tempdir();
        std::fs::write(directory.join("AGENTS.md"), "Use tabs, never spaces.").unwrap();
        let assembled = assemble(&directory, "add a function", &[]).await;
        let instructions = assembled
            .pinned
            .sections
            .iter()
            .find(|section| section.origin == PinnedOrigin::ProjectInstructions)
            .expect("AGENTS.md is attached");
        assert_eq!(instructions.label, "AGENTS.md");
        assert!(instructions.content.contains("Use tabs, never spaces."));
    }

    #[tokio::test]
    async fn the_same_reference_twice_is_attached_once() {
        let directory = tempdir();
        std::fs::write(directory.join("a.rs"), "fn a() {}").unwrap();
        let assembled = assemble(&directory, "compare @a.rs with @a.rs", &[]).await;
        assert_eq!(
            assembled
                .pinned
                .sections
                .iter()
                .filter(|section| section.origin == PinnedOrigin::ComposerReference)
                .count(),
            1
        );
    }

    /// A unique scratch directory. The daemon crate has no dev-dependency on a
    /// temp-dir helper, and adding one for four tests is more dependency than
    /// the tests are worth.
    fn tempdir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("purrcode-pctx-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }
}
