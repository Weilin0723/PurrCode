//! Creating, renaming, and deleting files from the Explorer.
//!
//! v1.2 Pillar 1: a developer must be able to shape a repository without
//! leaving PurrCode. These operations run against the local filesystem
//! directly, the same way the tree reads it and the editor writes it — a
//! network round trip for "make a folder" would buy nothing.
//!
//! Everything here is confined to the open folder. A path that resolves
//! outside it is refused rather than clamped: silently operating on a
//! different file than the one named is the failure mode worth designing out.

use egui::{RichText, Ui};

use super::PurrCodeIde;
use crate::theme;

/// A filesystem operation waiting on the user's confirmation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FileOperation {
    /// Create a file inside `parent`.
    NewFile { parent: std::path::PathBuf },
    /// Create a folder inside `parent`.
    NewFolder { parent: std::path::PathBuf },
    /// Rename `target` to a new name in the same directory.
    Rename { target: std::path::PathBuf },
    /// Delete `target`.
    Delete { target: std::path::PathBuf },
}

impl FileOperation {
    fn title(&self) -> &'static str {
        match self {
            Self::NewFile { .. } => "New file",
            Self::NewFolder { .. } => "New folder",
            Self::Rename { .. } => "Rename",
            Self::Delete { .. } => "Delete",
        }
    }

    /// The path this operation acts on, for the confinement check.
    fn anchor(&self) -> &std::path::Path {
        match self {
            Self::NewFile { parent } | Self::NewFolder { parent } => parent,
            Self::Rename { target } | Self::Delete { target } => target,
        }
    }
}

/// Why a filesystem operation was refused.
///
/// These are returned rather than logged so the dialog can say what went
/// wrong in the same place the user typed the name.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum FileError {
    EmptyName,
    /// The name contains a path separator or `..`, which would place the
    /// result somewhere other than where the user is looking.
    NameIsAPath,
    /// The resolved path is outside the open folder.
    OutsideWorkspace,
    AlreadyExists,
    Io(String),
}

impl FileError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::EmptyName => "Enter a name.".to_owned(),
            Self::NameIsAPath => {
                "A name cannot contain a path. Create the folder first, then the file inside it."
                    .to_owned()
            }
            Self::OutsideWorkspace => {
                "That path is outside the open folder. PurrCode only edits files inside it."
                    .to_owned()
            }
            Self::AlreadyExists => "Something with that name is already here.".to_owned(),
            Self::Io(detail) => detail.clone(),
        }
    }
}

/// Validates one path component typed by the user.
///
/// Rejects anything that is not a plain name: an empty string, a separator, a
/// `..`, or a drive/root prefix. This is what keeps "new file" from writing
/// outside the folder the user is looking at.
pub(crate) fn validate_name(name: &str) -> Result<&str, FileError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(FileError::EmptyName);
    }
    let path = std::path::Path::new(trimmed);
    let mut components = path.components();
    let only = components.next();
    if components.next().is_some() {
        return Err(FileError::NameIsAPath);
    }
    match only {
        Some(std::path::Component::Normal(_)) => Ok(trimmed),
        _ => Err(FileError::NameIsAPath),
    }
}

/// Confirms a path resolves inside the workspace.
///
/// The comparison is done on canonicalised ancestors so a symlink pointing out
/// of the folder is caught. The target itself may not exist yet (that is the
/// point, for a create), so its nearest existing ancestor is what gets checked.
pub(crate) fn confine(
    path: &std::path::Path,
    workspace: &std::path::Path,
) -> Result<(), FileError> {
    let Ok(root) = workspace.canonicalize() else {
        return Err(FileError::OutsideWorkspace);
    };
    let mut cursor = path;
    let resolved = loop {
        if let Ok(resolved) = cursor.canonicalize() {
            break resolved;
        }
        match cursor.parent() {
            Some(parent) => cursor = parent,
            // Walked to the filesystem root without finding anything real.
            None => return Err(FileError::OutsideWorkspace),
        }
    };
    if resolved.starts_with(&root) {
        Ok(())
    } else {
        Err(FileError::OutsideWorkspace)
    }
}

impl PurrCodeIde {
    /// Runs a pending operation with the name the user typed.
    pub(crate) fn commit_file_operation(
        &mut self,
        operation: &FileOperation,
        name: &str,
    ) -> Result<(), FileError> {
        let workspace = self.repository.clone();
        confine(operation.anchor(), &workspace)?;
        match operation {
            FileOperation::NewFile { parent } => {
                let name = validate_name(name)?;
                let target = parent.join(name);
                confine(&target, &workspace)?;
                if target.exists() {
                    return Err(FileError::AlreadyExists);
                }
                // `create_new` rather than `write`: between the check above and
                // this call another process could have created the file, and
                // truncating somebody else's new file would destroy it.
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .map_err(io_error)?;
                self.open_file(target);
                Ok(())
            }
            FileOperation::NewFolder { parent } => {
                let name = validate_name(name)?;
                let target = parent.join(name);
                confine(&target, &workspace)?;
                if target.exists() {
                    return Err(FileError::AlreadyExists);
                }
                std::fs::create_dir(&target).map_err(io_error)?;
                self.expanded.insert(parent.clone());
                Ok(())
            }
            FileOperation::Rename { target } => {
                let name = validate_name(name)?;
                let parent = target.parent().ok_or(FileError::OutsideWorkspace)?;
                let renamed = parent.join(name);
                confine(&renamed, &workspace)?;
                if renamed == *target {
                    return Ok(());
                }
                if renamed.exists() {
                    return Err(FileError::AlreadyExists);
                }
                std::fs::rename(target, &renamed).map_err(io_error)?;
                self.retarget_open_file(target, &renamed);
                Ok(())
            }
            FileOperation::Delete { target } => {
                let result = if target.is_dir() {
                    std::fs::remove_dir_all(target)
                } else {
                    std::fs::remove_file(target)
                };
                result.map_err(io_error)?;
                self.close_paths_under(target);
                Ok(())
            }
        }
    }

    /// Follows a rename in the editor, so the tab keeps pointing at the file.
    fn retarget_open_file(&mut self, from: &std::path::Path, to: &std::path::Path) {
        let label = to
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| to.display().to_string());
        for file in &mut self.open_files {
            if file.path == from {
                file.path = to.to_path_buf();
                file.label = label.clone();
                file.disk_stamp = super::disk_stamp(to);
            }
        }
        if self.dirty.remove(from) {
            self.dirty.insert(to.to_path_buf());
        }
        self.expanded.remove(from);
    }

    /// Closes every editor tab under a deleted path.
    ///
    /// A tab left open on a deleted file would save it back into existence on
    /// the next ⌘S, quietly undoing the delete.
    fn close_paths_under(&mut self, deleted: &std::path::Path) {
        self.open_files
            .retain(|file| !file.path.starts_with(deleted));
        self.dirty.retain(|path| !path.starts_with(deleted));
        self.expanded.retain(|path| !path.starts_with(deleted));
        self.active_file = self
            .active_file
            .min(self.open_files.len().saturating_sub(1));
    }

    /// The modal for the pending filesystem operation, if there is one.
    pub(crate) fn file_operation_dialog(&mut self, ctx: &egui::Context) {
        let Some(operation) = self.file_operation.clone() else {
            return;
        };
        let tokens = self.tokens;
        let mut close = false;
        let mut commit = false;
        let mut name = self.file_operation_name.clone();

        egui::Modal::new(egui::Id::new("purrcode_file_operation")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(
                RichText::new(operation.title())
                    .size(theme::TYPE_TITLE)
                    .color(tokens.text_primary),
            );
            ui.add_space(6.0);

            match &operation {
                FileOperation::Delete { target } => {
                    let what = if target.is_dir() { "folder" } else { "file" };
                    ui.label(
                        RichText::new(format!(
                            "Delete the {what} “{}”?",
                            target
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        ))
                        .color(tokens.text_primary),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(if target.is_dir() {
                            "Everything inside it is deleted too. This cannot be undone from \
                             PurrCode."
                        } else {
                            "This cannot be undone from PurrCode."
                        })
                        .size(theme::TYPE_META)
                        .color(tokens.text_secondary),
                    );
                }
                _ => {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut name)
                            .hint_text("Name")
                            .desired_width(f32::INFINITY),
                    );
                    response.request_focus();
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        commit = true;
                    }
                }
            }

            if let Some(error) = &self.file_operation_error {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(error)
                        .size(theme::TYPE_META)
                        .color(tokens.status_error),
                );
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                let confirm = match operation {
                    FileOperation::Delete { .. } => "Delete",
                    FileOperation::Rename { .. } => "Rename",
                    _ => "Create",
                };
                if ui.button(confirm).clicked() {
                    commit = true;
                }
            });
        });

        self.file_operation_name = name.clone();
        if commit {
            match self.commit_file_operation(&operation, &name) {
                Ok(()) => close = true,
                Err(error) => self.file_operation_error = Some(error.message()),
            }
        }
        if close {
            self.file_operation = None;
            self.file_operation_name.clear();
            self.file_operation_error = None;
        }
    }

    /// Starts an operation, clearing any previous attempt's error.
    pub(crate) fn begin_file_operation(&mut self, operation: FileOperation) {
        // Rename pre-fills the current name: the common case is changing part
        // of it, not typing it again from scratch.
        self.file_operation_name = match &operation {
            FileOperation::Rename { target } => target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            _ => String::new(),
        };
        self.file_operation_error = None;
        self.file_operation = Some(operation);
    }
}

/// The context menu shared by every row in the file tree.
pub(crate) fn tree_context_menu(
    ui: &mut Ui,
    path: &std::path::Path,
    is_directory: bool,
) -> Option<FileOperation> {
    let mut chosen = None;
    // A file's "new sibling" is created in its parent, which is what a user
    // right-clicking a file and choosing "New file" means.
    let parent = if is_directory {
        path.to_path_buf()
    } else {
        path.parent().map(std::path::Path::to_path_buf)?
    };
    if ui.button("New file…").clicked() {
        chosen = Some(FileOperation::NewFile {
            parent: parent.clone(),
        });
        ui.close();
    }
    if ui.button("New folder…").clicked() {
        chosen = Some(FileOperation::NewFolder { parent });
        ui.close();
    }
    ui.separator();
    if ui.button("Rename…").clicked() {
        chosen = Some(FileOperation::Rename {
            target: path.to_path_buf(),
        });
        ui.close();
    }
    if ui.button("Delete…").clicked() {
        chosen = Some(FileOperation::Delete {
            target: path.to_path_buf(),
        });
        ui.close();
    }
    ui.separator();
    if ui.button("Reveal in file manager").clicked() {
        reveal(path);
        ui.close();
    }
    chosen
}

/// Shows a path in the platform's file manager.
///
/// Spawned as a program with an argument, never as a shell string: a folder
/// whose name contains a quote must not become a command.
fn reveal(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let (program, args): (&str, Vec<&std::ffi::OsStr>) =
        ("open", vec!["-R".as_ref(), path.as_ref()]);
    #[cfg(target_os = "windows")]
    let (program, args): (&str, Vec<&std::ffi::OsStr>) = ("explorer", vec![path.as_ref()]);
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let (program, args): (&str, Vec<&std::ffi::OsStr>) =
        ("xdg-open", vec![path.parent().unwrap_or(path).as_ref()]);
    let _ = std::process::Command::new(program).args(args).spawn();
}

fn io_error(error: std::io::Error) -> FileError {
    FileError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_name_containing_a_path_is_refused() {
        // The whole point of the check: "../../etc/passwd" typed into a "new
        // file" box must not create a file two directories up.
        assert_eq!(validate_name("../escape"), Err(FileError::NameIsAPath));
        assert_eq!(validate_name("nested/file.rs"), Err(FileError::NameIsAPath));
        assert_eq!(validate_name(".."), Err(FileError::NameIsAPath));
        assert_eq!(validate_name("/absolute"), Err(FileError::NameIsAPath));
        assert_eq!(validate_name("   "), Err(FileError::EmptyName));
        assert_eq!(validate_name(""), Err(FileError::EmptyName));
    }

    #[test]
    fn an_ordinary_name_is_accepted_and_trimmed() {
        assert_eq!(validate_name("lib.rs"), Ok("lib.rs"));
        assert_eq!(validate_name("  notes.md  "), Ok("notes.md"));
        // A leading dot is a normal filename, not a traversal.
        assert_eq!(validate_name(".gitignore"), Ok(".gitignore"));
    }

    #[test]
    fn confinement_accepts_paths_inside_the_workspace_and_rejects_the_rest() {
        let workspace = tempfile::tempdir().expect("temp dir");
        let root = workspace.path();
        std::fs::create_dir(root.join("src")).expect("create src");

        assert_eq!(confine(&root.join("src"), root), Ok(()));
        // A file that does not exist yet is confined by its parent, which is
        // what makes "create" checkable at all.
        assert_eq!(confine(&root.join("src/new.rs"), root), Ok(()));
        assert_eq!(
            confine(Path::new("/etc/passwd"), root),
            Err(FileError::OutsideWorkspace)
        );
        assert_eq!(
            confine(&root.join("../outside.txt"), root),
            Err(FileError::OutsideWorkspace)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_pointing_out_of_the_workspace_is_refused() {
        // Component checking alone would pass this: every component is
        // "Normal". Only resolving the link catches it.
        let workspace = tempfile::tempdir().expect("temp dir");
        let outside = tempfile::tempdir().expect("temp dir");
        let link = workspace.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");

        assert_eq!(
            confine(&link.join("captured.txt"), workspace.path()),
            Err(FileError::OutsideWorkspace)
        );
    }
}
