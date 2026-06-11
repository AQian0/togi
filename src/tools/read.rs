use crate::common::{
    FileTooLargeError, IoErrorClass, ToolPathError, classify_io_error, is_binary, resolve_tool_path,
};
use crate::constants;
use crate::error::{ErrorKind, TogiError};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::{Path, PathBuf};

mod binary;
mod io;
mod text;

#[derive(Clone, Copy, Default)]
pub struct Read;

impl Read {
    fn resolve(cwd: Option<&Path>, raw_path: &str) -> Result<PathBuf, ReadError> {
        resolve_tool_path(cwd, raw_path).map_err(|e| match e {
            ToolPathError::EmptyPath => ReadError::EmptyPath,
            ToolPathError::MissingCwd => ReadError::MissingCwd,
        })
    }

    pub(super) fn map_io(source: std::io::Error, path: String) -> ReadError {
        match classify_io_error(&source) {
            IoErrorClass::NotFound => ReadError::NotFound { path },
            IoErrorClass::PermissionDenied => ReadError::PermissionDenied { path },
            IoErrorClass::NotUtf8 | IoErrorClass::Other => ReadError::Io { path, source },
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadArgs {
    /// Path of the file to read. May be absolute, or relative to the injected
    /// `cwd`. Do not pass a base directory or cwd manually.
    path: String,
    /// Current working directory (`cwd`). This is injected as hidden call context by
    /// `inject::inject`.
    #[serde(default)]
    #[schemars(skip)]
    cwd: Option<PathBuf>,
    /// Output encoding for binary files. Use `"hex"` (default) for a hexdump
    /// preview, or `"base64"` for the full base64-encoded content. Ignored for
    /// text files.
    #[serde(default)]
    encoding: Option<String>,
    /// Byte offset into the file to start reading from. Defaults to 0 (start).
    /// Use together with `limit_bytes` to read specific portions of large files.
    #[serde(default)]
    offset_bytes: Option<u64>,
    /// Maximum number of bytes to read from the file. Defaults to 50 KB for
    /// text files; binary files are always limited to the hexdump preview size.
    /// Pass 0 for no limit (capped by server memory — prefer using `shell` for
    /// truly large files).
    #[serde(default)]
    limit_bytes: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("`path` must not be empty.")]
    EmptyPath,
    #[error(
        "`cwd` was not injected. Register `read` through `inject::inject` with a \
         hidden `cwd` value."
    )]
    MissingCwd,
    #[error("no such file: `{path}`. Double-check the path, then retry.")]
    NotFound { path: String },
    #[error("`{path}` is a directory, not a file. Provide a path that points to a file.")]
    NotAFile { path: String },
    #[error("permission denied while reading `{path}`.")]
    PermissionDenied { path: String },
    #[error(transparent)]
    FileTooLarge(#[from] FileTooLargeError),
    #[error("unsupported encoding `{enc}`. Use `hex` or `base64`.")]
    InvalidEncoding { enc: String },
    #[error("io error while reading `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

impl TogiError for ReadError {
    fn code(&self) -> &'static str {
        match self {
            Self::EmptyPath => "tool.empty_path",
            Self::MissingCwd => "tool.missing_cwd",
            Self::NotFound { .. } => "read.not_found",
            Self::NotAFile { .. } => "read.not_a_file",
            Self::PermissionDenied { .. } => "read.permission_denied",
            Self::FileTooLarge(_) => "read.file_too_large",
            Self::InvalidEncoding { .. } => "read.invalid_encoding",
            Self::Io { .. } => "read.io",
        }
    }

    fn kind(&self) -> ErrorKind {
        match self {
            Self::EmptyPath => ErrorKind::InvalidArgument,
            Self::MissingCwd => ErrorKind::MissingRuntimeInjection,
            Self::NotFound { .. } => ErrorKind::NotFound,
            Self::NotAFile { .. } => ErrorKind::NotAFile,
            Self::PermissionDenied { .. } => ErrorKind::PermissionDenied,
            Self::FileTooLarge(_) => ErrorKind::TooLarge,
            Self::InvalidEncoding { .. } => ErrorKind::InvalidArgument,
            Self::Io { .. } => ErrorKind::Io,
        }
    }
}

impl Tool for Read {
    const NAME: &'static str = "read";
    type Error = ReadError;
    type Args = ReadArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let parameters = schemars::schema_for!(ReadArgs);
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read the contents of a file. Supports text files (returned with line \
                          numbers) and binary files (returned as a hexdump preview or base64). \
                          `path` may be absolute or relative to the injected `cwd`. For binary \
                          files the output includes file size and a hexdump of the first 512 \
                          bytes by default; pass `encoding: \"base64\"` to get the full \
                          base64-encoded content instead. For large text files (>10 MB) only the \
                          first 50 KB are returned by default; use `offset_bytes` and `limit_bytes` \
                          to read specific portions. Files larger than 100 MB are rejected — use \
                          the `shell` tool with commands like `head`, `tail`, or `sed` for those. \
                          On failure the tool returns a descriptive error explaining how to fix \
                          the call."
                .to_string(),
            parameters: serde_json::to_value(parameters).unwrap(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = Self::resolve(args.cwd.as_deref(), &args.path)?;
        let display = path.display().to_string();
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|source| Self::map_io(source, display.clone()))?;
        if metadata.is_dir() {
            return Err(ReadError::NotAFile { path: display });
        }
        let file_size = metadata.len();

        let _large_warning = crate::common::check_file_size(&display, file_size)?;

        let is_large = file_size > constants::LARGE_FILE_THRESHOLD;
        let offset_bytes = args.offset_bytes.unwrap_or(0);
        let remaining_bytes = file_size.saturating_sub(offset_bytes);
        let encoding = args.encoding.as_deref().unwrap_or("hex");

        let head = io::read_head(&path, file_size)
            .await
            .map_err(|source| Self::map_io(source, display.clone()))?;
        let binary = is_binary(&head);

        if binary {
            return binary::read_binary(binary::BinaryReadRequest {
                path: &path,
                display: &display,
                file_size,
                offset_bytes,
                remaining_bytes,
                is_large,
                encoding,
                limit_bytes: args.limit_bytes,
            })
            .await;
        }

        if is_large {
            let limit = args
                .limit_bytes
                .unwrap_or(constants::DEFAULT_MAX_READ_BYTES)
                .min(remaining_bytes);
            return text::read_streaming(&path, &display, file_size, offset_bytes, limit).await;
        }

        let buf = tokio::fs::read(&path)
            .await
            .map_err(|source| Self::map_io(source, display.clone()))?;

        let content = String::from_utf8(buf).map_err(|source| ReadError::Io {
            path: display,
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
        })?;
        Ok(text::render(&content))
    }
}
