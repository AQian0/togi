use crate::common::{
    FileTooLargeError, IoErrorClass, ToolPathError, classify_io_error, resolve_tool_path,
};
use crate::error::{ErrorKind, TogiError};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::Deserialize;
use std::io::ErrorKind as IoErrorKind;
use std::path::{Path, PathBuf};

mod atomic;
mod edit;
mod write;

use edit::Replacement;

#[derive(Deserialize, JsonSchema)]
struct EditInstruction {
    /// Exact text to search for. Must appear exactly once in the file, and its
    /// match must not overlap any other edit in the same call.
    old_text: String,
    /// Replacement text. When omitted or empty, the matched text is deleted.
    #[serde(default)]
    new_text: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ModifyArgs {
    /// Path of the file to modify. May be absolute, or relative to the injected
    /// `cwd`. Do not pass a base directory or cwd manually.
    path: String,
    /// Current working directory (`cwd`). This is injected as hidden call context by
    /// `inject::inject`.
    #[serde(default)]
    #[schemars(skip)]
    cwd: Option<PathBuf>,
    /// Full file content to write. When provided, the file is created (or
    /// completely overwritten) with this content. Cannot be combined with
    /// `old_text` / `new_text` / `edits`. Missing parent directories are
    /// created automatically.
    #[serde(default)]
    content: Option<String>,
    /// Exact text to search for when editing an existing file. Must appear
    /// exactly once in the file. Cannot be combined with `content`.
    #[serde(default)]
    old_text: Option<String>,
    /// Replacement text for `old_text`. When omitted or empty, the matched text
    /// is deleted. Cannot be combined with `content`.
    #[serde(default)]
    new_text: Option<String>,
    /// Multiple replacements applied to an existing file in a single call. Each
    /// entry's `old_text` must match exactly once in the original file, and the
    /// matches must not overlap. All matches are located against the original
    /// content. May be combined with a single `old_text` / `new_text`, but not
    /// with `content`.
    #[serde(default)]
    edits: Option<Vec<EditInstruction>>,
    /// Base64-encoded content to write to a file. Use this for binary files.
    /// When provided, the decoded bytes are written as-is. Cannot be combined
    /// with `content`, `old_text`, `new_text`, or `edits`.
    #[serde(default)]
    content_base64: Option<String>,
    /// When `true`, compute and return the diff without actually modifying the
    /// file. Defaults to `false`.
    #[serde(default)]
    dry_run: Option<bool>,
}

#[derive(Debug, thiserror::Error)]
pub enum ModifyError {
    #[error("`path` must not be empty.")]
    EmptyPath,
    #[error(
        "`cwd` was not injected. Register `modify` through `inject::inject` with a \
         hidden `cwd` value."
    )]
    MissingCwd,
    #[error(
        "no edit instructions provided. Pass `content` to write the whole file, or `old_text` \
         (and optional `new_text`) / `edits` to replace text in an existing file."
    )]
    NoInstructions,
    #[error(
        "conflicting instructions: `content` cannot be combined with `old_text`, `new_text`, or \
         `edits`. Pass `content` alone to overwrite the file, or use the edit fields alone."
    )]
    ConflictingInstructions,
    #[error(
        "`content_base64` cannot be combined with `content`, `old_text`, `new_text`, or `edits`. \
         For binary writes, pass `content_base64` alone."
    )]
    ConflictingBase64,
    #[error("`content_base64` is not valid base64: {source}")]
    InvalidBase64 {
        #[source]
        source: base64::DecodeError,
    },
    #[error("`old_text` must not be empty. Provide the exact text to replace.")]
    EmptyOldText,
    #[error(transparent)]
    FileTooLarge(#[from] FileTooLargeError),
    #[error("no such file: `{path}`. Double-check the path, then retry.")]
    NotFound { path: String },
    #[error("`{path}` is a directory, not a file. Provide a path that points to a file.")]
    NotAFile { path: String },
    #[error("{message}")]
    OldTextNotFound { path: String, message: String },
    #[error("{message}")]
    OldTextNotUnique { path: String, message: String },
    #[error(
        "two or more edits target overlapping text in `{path}`. Make each `old_text` cover a \
         distinct region."
    )]
    OverlappingEdits { path: String },
    #[error("permission denied while modifying `{path}`.")]
    PermissionDenied { path: String },
    #[error("`{path}` is not valid UTF-8 text and cannot be edited as a string.")]
    NotUtf8 { path: String },
    #[error("io error while modifying `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

impl edit::EditFailure for ModifyError {
    fn empty_old_text() -> Self {
        Self::EmptyOldText
    }

    fn old_text_not_found(path: String, message: String) -> Self {
        Self::OldTextNotFound { path, message }
    }

    fn old_text_not_unique(path: String, message: String) -> Self {
        Self::OldTextNotUnique { path, message }
    }

    fn overlapping_edits(path: String) -> Self {
        Self::OverlappingEdits { path }
    }
}

impl atomic::AtomicWriteFailure for ModifyError {
    fn from_atomic_io(source: std::io::Error, display: &str) -> Self {
        Modify::map_io(source, display)
    }
}

impl TogiError for ModifyError {
    fn code(&self) -> &'static str {
        match self {
            Self::EmptyPath => "tool.empty_path",
            Self::MissingCwd => "tool.missing_cwd",
            Self::NoInstructions => "modify.no_instructions",
            Self::ConflictingInstructions => "modify.conflicting_instructions",
            Self::ConflictingBase64 => "modify.conflicting_base64",
            Self::InvalidBase64 { .. } => "modify.invalid_base64",
            Self::EmptyOldText => "modify.empty_old_text",
            Self::FileTooLarge(_) => "modify.file_too_large",
            Self::NotFound { .. } => "modify.not_found",
            Self::NotAFile { .. } => "modify.not_a_file",
            Self::OldTextNotFound { .. } => "modify.old_text_not_found",
            Self::OldTextNotUnique { .. } => "modify.old_text_not_unique",
            Self::OverlappingEdits { .. } => "modify.overlapping_edits",
            Self::PermissionDenied { .. } => "modify.permission_denied",
            Self::NotUtf8 { .. } => "modify.not_utf8",
            Self::Io { .. } => "modify.io",
        }
    }

    fn kind(&self) -> ErrorKind {
        match self {
            Self::EmptyPath
            | Self::NoInstructions
            | Self::InvalidBase64 { .. }
            | Self::EmptyOldText => ErrorKind::InvalidArgument,
            Self::MissingCwd => ErrorKind::MissingRuntimeInjection,
            Self::ConflictingInstructions
            | Self::ConflictingBase64
            | Self::OldTextNotFound { .. }
            | Self::OldTextNotUnique { .. }
            | Self::OverlappingEdits { .. } => ErrorKind::Conflict,
            Self::FileTooLarge(_) => ErrorKind::TooLarge,
            Self::NotFound { .. } => ErrorKind::NotFound,
            Self::NotAFile { .. } => ErrorKind::NotAFile,
            Self::PermissionDenied { .. } => ErrorKind::PermissionDenied,
            Self::NotUtf8 { .. } => ErrorKind::NotUtf8,
            Self::Io { .. } => ErrorKind::Io,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Modify;

impl Modify {
    fn resolve(cwd: Option<&Path>, raw_path: &str) -> Result<PathBuf, ModifyError> {
        resolve_tool_path(cwd, raw_path).map_err(|e| match e {
            ToolPathError::EmptyPath => ModifyError::EmptyPath,
            ToolPathError::MissingCwd => ModifyError::MissingCwd,
        })
    }

    async fn resolve_symlinks(path: &Path) -> Result<PathBuf, ModifyError> {
        match tokio::fs::canonicalize(path).await {
            Ok(resolved) => Ok(resolved),
            Err(source) if source.kind() == IoErrorKind::NotFound => {
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    let resolved_parent =
                        tokio::fs::canonicalize(parent)
                            .await
                            .map_err(|e| ModifyError::Io {
                                path: path.display().to_string(),
                                source: e,
                            })?;
                    Ok(resolved_parent.join(path.file_name().unwrap()))
                } else {
                    Ok(path.to_path_buf())
                }
            }
            Err(source) => Err(ModifyError::Io {
                path: path.display().to_string(),
                source,
            }),
        }
    }

    pub(super) fn map_io(source: std::io::Error, display: &str) -> ModifyError {
        let path = display.to_string();
        match classify_io_error(&source) {
            IoErrorClass::NotFound => ModifyError::NotFound { path },
            IoErrorClass::PermissionDenied => ModifyError::PermissionDenied { path },
            IoErrorClass::NotUtf8 => ModifyError::NotUtf8 { path },
            IoErrorClass::Other => ModifyError::Io { path, source },
        }
    }
}

impl Tool for Modify {
    const NAME: &'static str = "modify";
    type Error = ModifyError;
    type Args = ModifyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let parameters = schemars::schema_for!(ModifyArgs);
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Modify or write a file. `path` may be absolute or relative to the \
                          injected `cwd` (relative paths are resolved \
                          automatically). Three modes: pass `content` to create or completely \
                          overwrite a text file (missing parent directories are created); \
                          pass `content_base64` to write binary content; or edit an \
                          existing file by passing `old_text` (and optional `new_text`) and/or an \
                          `edits` array for multiple replacements. Each `old_text` must match \
                          exactly once in the original file and the matches must not overlap. \
                          `content`, `content_base64`, and the edit fields are mutually exclusive. \
                          Writes are atomic (temp file + rename), so a failed write never \
                          corrupts the original. Use `dry_run: true` to preview changes \
                          without modifying. On failure the tool returns a descriptive error \
                          explaining how to fix the call."
                .to_string(),
            parameters: serde_json::to_value(parameters).unwrap(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let raw_path = Self::resolve(args.cwd.as_deref(), &args.path)?;
        let path = Self::resolve_symlinks(&raw_path).await?;
        let display = raw_path.display().to_string();
        let dry_run = args.dry_run.unwrap_or(false);

        if let Some(b64) = args.content_base64.as_deref() {
            let has_conflict = args.content.is_some()
                || args.old_text.is_some()
                || args.new_text.is_some()
                || args.edits.is_some();
            if has_conflict {
                return Err(ModifyError::ConflictingBase64);
            }
            use base64::Engine;
            let data = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|source| ModifyError::InvalidBase64 { source })?;
            return write::write_binary_file(&path, &display, &data, dry_run).await;
        }

        let has_edit_fields =
            args.old_text.is_some() || args.new_text.is_some() || args.edits.is_some();
        if args.content.is_some() && has_edit_fields {
            return Err(ModifyError::ConflictingInstructions);
        }
        if let Some(content) = args.content.as_deref() {
            return write::write_text_file(&path, &display, content, dry_run).await;
        }

        let mut replacements: Vec<Replacement<'_>> = Vec::new();
        if let Some(edits) = args.edits.as_deref() {
            for edit in edits {
                replacements.push(Replacement {
                    old: edit.old_text.as_str(),
                    new: edit.new_text.as_deref().unwrap_or(""),
                });
            }
        }
        if let Some(old_text) = args.old_text.as_deref() {
            replacements.push(Replacement {
                old: old_text,
                new: args.new_text.as_deref().unwrap_or(""),
            });
        }
        if replacements.is_empty() {
            return Err(ModifyError::NoInstructions);
        }
        edit::edit_file(&path, &display, &replacements, dry_run).await
    }
}
