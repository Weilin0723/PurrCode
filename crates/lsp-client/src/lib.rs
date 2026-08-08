//! Dependency-light Language Server Protocol client.
//!
//! PurrCode is an **LSP client**, not a language server: it spawns the
//! platform language server (rust-analyzer, typescript-language-server, …) and
//! speaks LSP JSON-RPC over stdio with `Content-Length` framing. This crate
//! implements the client side — lifecycle, diagnostics, hover, definition,
//! references, symbols, and formatting — with no new dependencies beyond the
//! workspace's existing serde/tokio stack.
//!
//! A server is spawned once per language and kept alive for the session; each
//! request is a JSON-RPC call with a fresh id. The protocol types are kept
//! intentionally small and match the LSP 3.17 wire shape.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

/// A language server command for a file extension, resolved from PATH.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LanguageServerCommand {
    /// Bare program name, e.g. `rust-analyzer`.
    pub program: String,
    /// Arguments, e.g. `["--stdio"]` or `["--stdio", "--no-lsp"]`.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// File extensions this server serves, e.g. `["rs"]`.
    pub extensions: Vec<String>,
}

/// The canonical server commands PurrCode knows about. Any entry whose program
/// is missing from PATH is skipped, so a machine without rust-analyzer simply
/// has no Rust intelligence instead of a broken feature.
pub fn default_server_commands() -> Vec<LanguageServerCommand> {
    vec![
        LanguageServerCommand {
            program: "rust-analyzer".into(),
            arguments: vec![],
            extensions: vec!["rs".into()],
        },
        LanguageServerCommand {
            program: "typescript-language-server".into(),
            arguments: vec!["--stdio".into()],
            extensions: vec!["ts".into(), "tsx".into(), "js".into(), "jsx".into()],
        },
        LanguageServerCommand {
            program: "pyright-langserver".into(),
            arguments: vec!["--stdio".into()],
            extensions: vec!["py".into()],
        },
        LanguageServerCommand {
            program: "gopls".into(),
            arguments: vec![],
            extensions: vec!["go".into()],
        },
    ]
}

/// A position in a document (0-based line, 0-based character).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Position {
    pub line: u64,
    pub character: u64,
}

/// A range between two positions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// A request to ask the language server about a document location.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocationRequest {
    /// Absolute path of the document.
    pub path: PathBuf,
    /// The cursor position.
    pub position: Position,
}

/// A hover response: the rendered markdown plus the range it covers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HoverResult {
    pub contents: String,
    pub range: Option<Range>,
}

/// A symbol in a document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: u64,
    pub detail: Option<String>,
    pub range: Range,
    pub selection_range: Range,
}

/// A location link: the target document and range.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocationLink {
    pub target_uri: String,
    pub target_range: Range,
    pub target_selection_range: Range,
}

/// A formatting edit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

/// A symbol found by a workspace-wide symbol query.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: u64,
    pub container: Option<String>,
    pub uri: String,
    pub range: Range,
}

/// One diagnostic published by a language server for a document.
///
/// `severity` follows the LSP scale (1 = error, 2 = warning, 3 = information,
/// 4 = hint). It is `None` when the server omitted it, which the UI must show
/// as "unspecified" rather than silently promoting to an error.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Option<u64>,
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
}

/// The diagnostics a server has published for one document, with the document
/// it belongs to. Diagnostics arrive as unsolicited notifications, so this is a
/// snapshot of what has arrived so far — never a promise of completeness.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileDiagnostics {
    pub path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
}

/// A rename result: the edits to apply, grouped by the file they belong to.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceEdit {
    /// Absolute path → the edits for that file, in the order the server gave.
    pub changes: BTreeMap<PathBuf, Vec<TextEdit>>,
}

#[derive(Debug, Error)]
pub enum LspError {
    #[error("no language server is configured for extension `{0}`")]
    UnsupportedExtension(String),
    #[error("language server `{0}` is not available on this machine")]
    ServerUnavailable(String),
    #[error("language server for `{0}` exited unexpectedly")]
    ServerExited(String),
    #[error("LSP JSON-RPC returned an error: {0}")]
    Rpc(String),
    #[error("LSP response was malformed: {0}")]
    Malformed(String),
    #[error("I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization failure: {0}")]
    Json(#[from] serde_json::Error),
    #[error("document `{0}` does not exist")]
    MissingDocument(PathBuf),
}

/// A live language server for one language, identified by its root URI.
pub struct LspServer {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    root_uri: String,
    next_id: u64,
    /// The latest `textDocument/publishDiagnostics` payload per document URI.
    ///
    /// Diagnostics are unsolicited notifications, so they are captured
    /// opportunistically whenever the transport is read and cached here. A
    /// server republishes the full set for a document on every change, so
    /// storing the newest payload per URI (rather than appending) matches the
    /// protocol: an empty array means "this file is now clean".
    diagnostics: BTreeMap<String, Vec<Diagnostic>>,
}

impl Drop for LspServer {
    fn drop(&mut self) {
        // Best-effort kill; the child also has kill_on_drop=true so it cannot
        // leak beyond this process.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.start_kill();
        }
    }
}

/// Spawns a language server for a file and holds it open for the session.
pub async fn start_server(
    file: &Path,
    root: &Path,
    commands: &[LanguageServerCommand],
) -> Result<LspServer, LspError> {
    let extension = file
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let command = commands
        .iter()
        .find(|candidate| candidate.extensions.iter().any(|ext| ext == &extension))
        .ok_or_else(|| LspError::UnsupportedExtension(extension.clone()))?;
    let mut process = Command::new(&command.program)
        .args(&command.arguments)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| LspError::ServerUnavailable(command.program.clone()))?;
    let stdin = process
        .stdin
        .take()
        .ok_or_else(|| LspError::ServerExited(command.program.clone()))?;
    let stdout = process
        .stdout
        .take()
        .ok_or_else(|| LspError::ServerExited(command.program.clone()))?;
    let mut server = LspServer {
        child: process,
        stdin,
        stdout: BufReader::new(stdout),
        root_uri: path_to_uri(root),
        next_id: 1,
        diagnostics: BTreeMap::new(),
    };
    server.initialize().await?;
    Ok(server)
}

/// The root URI for a path, matching LSP's `file://` scheme.
pub fn path_to_uri(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    format!("file://{}", path.display())
}

/// A file URI back to a local path.
pub fn uri_to_path(uri: &str) -> PathBuf {
    PathBuf::from(uri.strip_prefix("file://").unwrap_or(uri))
}

impl LspServer {
    async fn initialize(&mut self) -> Result<(), LspError> {
        let result = self
            .call(
                "initialize",
                json!({
                    "processId": null,
                    "rootUri": self.root_uri,
                    "capabilities": {},
                    "workspaceFolders": [{"uri": self.root_uri, "name": "purrcode"}],
                }),
            )
            .await?;
        // Send the initialized notification so the server starts publishing
        // diagnostics rather than waiting for a second initialize.
        self.notify("initialized", json!({})).await?;
        let _ = result;
        Ok(())
    }

    /// Opens a document in the server so it starts producing diagnostics.
    pub async fn did_open(
        &mut self,
        path: &Path,
        language_id: &str,
        text: &str,
    ) -> Result<(), LspError> {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": path_to_uri(path),
                    "languageId": language_id,
                    "version": 1,
                    "text": text,
                }
            }),
        )
        .await
    }

    /// Sends the current document content after an edit.
    pub async fn did_change(&mut self, path: &Path, text: &str) -> Result<(), LspError> {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": path_to_uri(path), "version": 2},
                "contentChanges": [{"text": text}],
            }),
        )
        .await
    }

    /// Requests hover text at a position.
    pub async fn hover(
        &mut self,
        path: &Path,
        position: Position,
    ) -> Result<HoverResult, LspError> {
        let result = self
            .call(
                "textDocument/hover",
                json!({
                    "textDocument": {"uri": path_to_uri(path)},
                    "position": position,
                }),
            )
            .await?;
        let contents = extract_hover_contents(&result["contents"])?;
        Ok(HoverResult {
            contents,
            range: serde_json::from_value(result["range"].clone()).ok(),
        })
    }

    /// Resolves the definition of the symbol at a position.
    pub async fn definition(
        &mut self,
        path: &Path,
        position: Position,
    ) -> Result<Vec<LocationLink>, LspError> {
        let result = self
            .call(
                "textDocument/definition",
                json!({
                    "textDocument": {"uri": path_to_uri(path)},
                    "position": position,
                }),
            )
            .await?;
        parse_locations(result)
    }

    /// Finds references to the symbol at a position across the workspace.
    pub async fn references(
        &mut self,
        path: &Path,
        position: Position,
    ) -> Result<Vec<LocationLink>, LspError> {
        let result = self
            .call(
                "textDocument/references",
                json!({
                    "textDocument": {"uri": path_to_uri(path)},
                    "position": position,
                    "context": {"includeDeclaration": true},
                }),
            )
            .await?;
        parse_locations(result)
    }

    /// Lists the top-level symbols in a document.
    pub async fn document_symbols(&mut self, path: &Path) -> Result<Vec<DocumentSymbol>, LspError> {
        let result = self
            .call(
                "textDocument/documentSymbol",
                json!({"textDocument": {"uri": path_to_uri(path)}}),
            )
            .await?;
        let symbols = match result {
            Value::Array(items) => items,
            Value::Null => Vec::new(),
            _ => Vec::new(),
        };
        symbols
            .into_iter()
            .map(|item| {
                serde_json::from_value::<DocumentSymbol>(item)
                    .map_err(|error| LspError::Malformed(error.to_string()))
            })
            .collect()
    }

    /// Requests full-document formatting edits.
    pub async fn formatting(&mut self, path: &Path) -> Result<Vec<TextEdit>, LspError> {
        let result = self
            .call(
                "textDocument/formatting",
                json!({
                    "textDocument": {"uri": path_to_uri(path)},
                    "options": {"tabSize": 4, "insertSpaces": true},
                }),
            )
            .await?;
        let edits = match result {
            Value::Array(items) => items,
            Value::Null => Vec::new(),
            _ => Vec::new(),
        };
        edits
            .into_iter()
            .map(|edit| {
                serde_json::from_value::<TextEdit>(edit)
                    .map_err(|error| LspError::Malformed(error.to_string()))
            })
            .collect()
    }

    /// Sends a JSON-RPC request and returns the `result` object.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, LspError> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        let response = self.read_response().await?;
        if response["id"] != id {
            return Err(LspError::Rpc("response id mismatch".into()));
        }
        if let Some(error) = response.get("error") {
            return Err(LspError::Rpc(error.to_string()));
        }
        Ok(response["result"].clone())
    }

    /// Sends a JSON-RPC notification (no response expected).
    async fn notify(&mut self, method: &str, params: Value) -> Result<(), LspError> {
        self.write(&json!({"jsonrpc":"2.0","method":method,"params":params}))
            .await
    }

    async fn write(&mut self, message: &Value) -> Result<(), LspError> {
        let body = serde_json::to_vec(message)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.stdin.write_all(header.as_bytes()).await?;
        self.stdin.write_all(&body).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Reads one JSON-RPC response, absorbing server-initiated notifications
    /// (e.g. `window/logMessage`, `textDocument/publishDiagnostics`) as they
    /// go past.
    async fn read_response(&mut self) -> Result<Value, LspError> {
        loop {
            let value = self.read_message().await?;
            // A message without an `id` is a server notification, not the
            // response to our request; record it and keep reading.
            if value.get("id").is_some() {
                return Ok(value);
            }
            self.absorb_notification(&value);
        }
    }

    /// Reads exactly one framed JSON-RPC message from the server.
    async fn read_message(&mut self) -> Result<Value, LspError> {
        let mut header = String::new();
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line).await? == 0 {
                return Err(LspError::ServerExited("language server".into()));
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            header.push_str(&line);
        }
        let content_length = header
            .lines()
            .find_map(|line| {
                let line = line.trim();
                line.strip_prefix("Content-Length:").map(str::trim)
            })
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| LspError::Malformed("missing Content-Length header".into()))?;
        let mut body = vec![0_u8; content_length];
        self.stdout.read_exact(&mut body).await?;
        serde_json::from_slice(&body).map_err(|error| LspError::Malformed(error.to_string()))
    }

    /// Files a server notification. Only `publishDiagnostics` is retained;
    /// everything else (log messages, progress) is deliberately dropped rather
    /// than surfaced as if it were a finding.
    fn absorb_notification(&mut self, value: &Value) {
        if value["method"] != "textDocument/publishDiagnostics" {
            return;
        }
        let Some(uri) = value["params"]["uri"].as_str() else {
            return;
        };
        let diagnostics = value["params"]["diagnostics"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| parse_diagnostic(item).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // A republish replaces the previous set for that document: an empty
        // array is the server saying the file is now clean, not "no news".
        self.diagnostics.insert(uri.to_owned(), diagnostics);
    }

    /// Drains whatever the server has already sent, without waiting for more.
    ///
    /// Diagnostics are pushed by the server on its own schedule, so a caller
    /// asking "what is wrong with this file" has to collect what has landed
    /// rather than issue a request. `budget` bounds the wait for the *first*
    /// byte of each message so a quiet server returns promptly instead of
    /// blocking the daemon's request thread.
    pub async fn pump_notifications(&mut self, budget: std::time::Duration) {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return;
            }
            // `fill_buf` is cancellation-safe for `BufReader`: any bytes it
            // read land in the reader's own buffer, so timing out here cannot
            // desynchronise the `Content-Length` framing the way timing out
            // mid-`read_exact` would.
            let ready = tokio::time::timeout(remaining, self.stdout.fill_buf()).await;
            match ready {
                Ok(Ok(buffer)) if !buffer.is_empty() => {}
                // Timed out, EOF, or an I/O error: nothing more to collect.
                _ => return,
            }
            match self.read_message().await {
                Ok(value) if value.get("id").is_none() => self.absorb_notification(&value),
                // A stray response with no outstanding request is discarded;
                // an error means the transport is finished for this pump.
                Ok(_) => {}
                Err(_) => return,
            }
        }
    }

    /// The diagnostics currently known for one document.
    pub fn diagnostics_for(&self, path: &Path) -> Vec<Diagnostic> {
        self.diagnostics
            .get(&path_to_uri(path))
            .cloned()
            .unwrap_or_default()
    }

    /// Every document this server has published diagnostics for.
    pub fn all_diagnostics(&self) -> Vec<FileDiagnostics> {
        self.diagnostics
            .iter()
            .map(|(uri, diagnostics)| FileDiagnostics {
                path: uri_to_path(uri),
                diagnostics: diagnostics.clone(),
            })
            .collect()
    }

    /// Renames the symbol at a position across the workspace.
    pub async fn rename(
        &mut self,
        path: &Path,
        position: Position,
        new_name: &str,
    ) -> Result<WorkspaceEdit, LspError> {
        let result = self
            .call(
                "textDocument/rename",
                json!({
                    "textDocument": {"uri": path_to_uri(path)},
                    "position": position,
                    "newName": new_name,
                }),
            )
            .await?;
        parse_workspace_edit(&result)
    }

    /// Queries the whole workspace for symbols matching `query`.
    pub async fn workspace_symbols(
        &mut self,
        query: &str,
    ) -> Result<Vec<WorkspaceSymbol>, LspError> {
        let result = self
            .call("workspace/symbol", json!({ "query": query }))
            .await?;
        let items = match result {
            Value::Array(items) => items,
            _ => Vec::new(),
        };
        Ok(items.iter().filter_map(parse_workspace_symbol).collect())
    }
}

fn parse_diagnostic(item: &Value) -> Result<Diagnostic, LspError> {
    let range = serde_json::from_value::<Range>(item["range"].clone())
        .map_err(|error| LspError::Malformed(error.to_string()))?;
    // `code` is `string | integer` on the wire; render both as text so the UI
    // has one type to display.
    let code = match &item["code"] {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    };
    Ok(Diagnostic {
        range,
        severity: item["severity"].as_u64(),
        code,
        source: item["source"].as_str().map(str::to_owned),
        message: item["message"].as_str().unwrap_or_default().to_owned(),
    })
}

/// Parses a `WorkspaceEdit`, accepting both the `changes` map and the newer
/// `documentChanges` array that most servers now send for renames.
fn parse_workspace_edit(result: &Value) -> Result<WorkspaceEdit, LspError> {
    let mut edit = WorkspaceEdit::default();
    if let Some(changes) = result["changes"].as_object() {
        for (uri, edits) in changes {
            let parsed = parse_text_edits(edits);
            if !parsed.is_empty() {
                edit.changes
                    .entry(uri_to_path(uri))
                    .or_default()
                    .extend(parsed);
            }
        }
    }
    if let Some(documents) = result["documentChanges"].as_array() {
        for document in documents {
            // Only plain text-document edits are applied. Create/rename/delete
            // file operations are deliberately ignored rather than half-applied:
            // a rename that silently moved files would be worse than one that
            // reports it only changed text.
            let Some(uri) = document["textDocument"]["uri"].as_str() else {
                continue;
            };
            let parsed = parse_text_edits(&document["edits"]);
            if !parsed.is_empty() {
                edit.changes
                    .entry(uri_to_path(uri))
                    .or_default()
                    .extend(parsed);
            }
        }
    }
    Ok(edit)
}

fn parse_text_edits(value: &Value) -> Vec<TextEdit> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<TextEdit>(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Parses one `SymbolInformation` or `WorkspaceSymbol` entry.
fn parse_workspace_symbol(item: &Value) -> Option<WorkspaceSymbol> {
    let location = &item["location"];
    let uri = location["uri"].as_str()?;
    let range = serde_json::from_value::<Range>(location["range"].clone()).ok()?;
    Some(WorkspaceSymbol {
        name: item["name"].as_str()?.to_owned(),
        kind: item["kind"].as_u64().unwrap_or(0),
        container: item["containerName"].as_str().map(str::to_owned),
        uri: uri.to_owned(),
        range,
    })
}

fn extract_hover_contents(contents: &Value) -> Result<String, LspError> {
    match contents {
        Value::String(text) => Ok(text.clone()),
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                if let Some(text) = part["value"].as_str() {
                    out.push_str(text);
                    out.push('\n');
                } else if let Some(text) = part.as_str() {
                    out.push_str(text);
                    out.push('\n');
                }
            }
            Ok(out.trim_end().to_string())
        }
        Value::Object(map) => Ok(map
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()),
        _ => Ok(String::new()),
    }
}

/// Parses `Location[] | LocationLink[] | Location | null` into links.
fn parse_locations(result: Value) -> Result<Vec<LocationLink>, LspError> {
    let items: Vec<Value> = match result {
        Value::Array(items) => items,
        Value::Null => Vec::new(),
        Value::Object(item) => vec![Value::Object(item)],
        _ => Vec::new(),
    };
    let mut links = Vec::new();
    for item in items {
        // LocationLink has targetUri; Location has uri + range.
        let (target_uri, target_range) = if let Some(uri) = item["targetUri"].as_str() {
            (
                uri.to_string(),
                serde_json::from_value::<Range>(item["targetRange"].clone())
                    .map_err(|error| LspError::Malformed(error.to_string()))?,
            )
        } else {
            let uri = item["uri"]
                .as_str()
                .ok_or_else(|| LspError::Malformed("location missing uri".into()))?;
            (
                uri.to_string(),
                serde_json::from_value::<Range>(item["range"].clone())
                    .map_err(|error| LspError::Malformed(error.to_string()))?,
            )
        };
        links.push(LocationLink {
            target_uri,
            target_range: target_range.clone(),
            target_selection_range: item["targetSelectionRange"]
                .clone()
                .pipe(|value| serde_json::from_value::<Range>(value).unwrap_or(target_range)),
        });
    }
    Ok(links)
}

trait Pipe: Sized {
    fn pipe<F, R>(self, f: F) -> R
    where
        F: FnOnce(Self) -> R,
    {
        f(self)
    }
}
impl<T> Pipe for T {}

/// A capability probe: which language servers are actually available on this
/// machine, used by the IDE settings to show what intelligence is enabled.
pub fn available_servers(commands: &[LanguageServerCommand]) -> Vec<LanguageServerCommand> {
    commands
        .iter()
        .filter(|command| {
            std::env::var_os("PATH").is_some_and(|paths| {
                std::env::split_paths(&paths).any(|dir| dir.join(&command.program).is_file())
            })
        })
        .cloned()
        .collect()
}

/// A table of language servers mapped to a document path, for reuse across
/// requests in one session.
#[derive(Default)]
pub struct LspManager {
    servers: BTreeMap<String, LspServer>,
    commands: Vec<LanguageServerCommand>,
}

impl LspManager {
    pub fn new(commands: Vec<LanguageServerCommand>) -> Self {
        Self {
            servers: BTreeMap::new(),
            commands,
        }
    }

    fn key(file: &Path) -> String {
        file.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
    }

    async fn server_for(&mut self, file: &Path, root: &Path) -> Result<&mut LspServer, LspError> {
        let key = Self::key(file);
        if !self.servers.contains_key(&key) {
            let server = start_server(file, root, &self.commands).await?;
            self.servers.insert(key.clone(), server);
        }
        Ok(self
            .servers
            .get_mut(&key)
            .expect("server was just inserted"))
    }

    pub async fn open(
        &mut self,
        file: &Path,
        root: &Path,
        language_id: &str,
        text: &str,
    ) -> Result<(), LspError> {
        let server = self.server_for(file, root).await?;
        server.did_open(file, language_id, text).await
    }

    pub async fn hover(
        &mut self,
        file: &Path,
        root: &Path,
        position: Position,
    ) -> Result<HoverResult, LspError> {
        let server = self.server_for(file, root).await?;
        server.hover(file, position).await
    }

    pub async fn definition(
        &mut self,
        file: &Path,
        root: &Path,
        position: Position,
    ) -> Result<Vec<LocationLink>, LspError> {
        let server = self.server_for(file, root).await?;
        server.definition(file, position).await
    }

    pub async fn references(
        &mut self,
        file: &Path,
        root: &Path,
        position: Position,
    ) -> Result<Vec<LocationLink>, LspError> {
        let server = self.server_for(file, root).await?;
        server.references(file, position).await
    }

    pub async fn symbols(
        &mut self,
        file: &Path,
        root: &Path,
    ) -> Result<Vec<DocumentSymbol>, LspError> {
        let server = self.server_for(file, root).await?;
        server.document_symbols(file).await
    }

    pub async fn format(&mut self, file: &Path, root: &Path) -> Result<Vec<TextEdit>, LspError> {
        let server = self.server_for(file, root).await?;
        server.formatting(file).await
    }

    pub async fn rename(
        &mut self,
        file: &Path,
        root: &Path,
        position: Position,
        new_name: &str,
    ) -> Result<WorkspaceEdit, LspError> {
        let server = self.server_for(file, root).await?;
        server.rename(file, position, new_name).await
    }

    pub async fn workspace_symbols(
        &mut self,
        file: &Path,
        root: &Path,
        query: &str,
    ) -> Result<Vec<WorkspaceSymbol>, LspError> {
        let server = self.server_for(file, root).await?;
        server.workspace_symbols(query).await
    }

    /// Collects the diagnostics a server has published for one document.
    ///
    /// `budget` is how long to wait for in-flight notifications before
    /// answering. Analysis is asynchronous in every real language server, so
    /// an empty result shortly after opening a file means "nothing has been
    /// published yet", not "this file is clean" — the caller reports the
    /// distinction rather than inventing a clean bill of health.
    pub async fn diagnostics(
        &mut self,
        file: &Path,
        root: &Path,
        budget: std::time::Duration,
    ) -> Result<Vec<Diagnostic>, LspError> {
        let server = self.server_for(file, root).await?;
        server.pump_notifications(budget).await;
        Ok(server.diagnostics_for(file))
    }

    /// Every document with published diagnostics, across all live servers.
    pub async fn all_diagnostics(&mut self, budget: std::time::Duration) -> Vec<FileDiagnostics> {
        let mut out = Vec::new();
        // The budget is per server: each one is a separate process with its
        // own pending notifications, and a shared budget would starve the
        // servers that happen to be polled last.
        for server in self.servers.values_mut() {
            server.pump_notifications(budget).await;
            out.extend(server.all_diagnostics());
        }
        out
    }

    pub fn drop_server(&mut self, file: &Path) {
        let key = Self::key(file);
        self.servers.remove(&key);
    }

    /// Which configured servers are actually on PATH, for the settings surface.
    pub fn available_servers(&self) -> Vec<LanguageServerCommand> {
        available_servers(&self.commands)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_and_uri_round_trip() {
        let path = PathBuf::from("/home/user/src/main.rs");
        let uri = path_to_uri(&path);
        assert!(uri.starts_with("file://"));
        assert_eq!(uri_to_path(&uri), path);
    }

    #[test]
    fn server_commands_are_filtered_to_available_ones() {
        // The probe reads PATH; this only asserts the shape, not availability.
        let commands = default_server_commands();
        assert!(!commands.is_empty());
        assert!(
            commands
                .iter()
                .any(|command| command.extensions.contains(&"rs".to_string()))
        );
    }

    #[tokio::test]
    async fn unsupported_extension_is_rejected_before_spawning() {
        let commands = default_server_commands();
        // A .png file has no server and start_server must fail before I/O.
        let result = start_server(&PathBuf::from("a.png"), &PathBuf::from("/"), &commands).await;
        assert!(matches!(
            result.as_ref(),
            Err(LspError::UnsupportedExtension(_))
        ));
    }
}
