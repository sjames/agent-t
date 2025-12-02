//! Rust Analyzer LSP client
//!
//! This module provides an LSP client for communicating with rust-analyzer,
//! enabling code intelligence features like diagnostics, goto definition,
//! find references, hover information, and more.

use anyhow::{anyhow, Result};
use lsp_types::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, RwLock};

/// JSON-RPC message types
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Message {
    Request(RequestMessage),
    Response(ResponseMessage),
    Notification(NotificationMessage),
}

#[derive(Debug, Serialize, Deserialize)]
struct RequestMessage {
    jsonrpc: String,
    id: i32,
    method: String,
    params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ResponseMessage {
    jsonrpc: String,
    id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ResponseError>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ResponseError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct NotificationMessage {
    jsonrpc: String,
    method: String,
    params: Option<Value>,
}

/// Pending request tracker
struct PendingRequest {
    tx: tokio::sync::oneshot::Sender<Result<Value>>,
}

/// Rust Analyzer LSP client
pub struct RustAnalyzerClient {
    /// The rust-analyzer process
    process: Arc<Mutex<Option<Child>>>,
    /// Process stdin
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    /// Next request ID
    next_id: Arc<AtomicI32>,
    /// Pending requests
    pending: Arc<RwLock<HashMap<i32, PendingRequest>>>,
    /// Workspace root
    workspace_root: PathBuf,
    /// Whether the server is initialized
    initialized: Arc<RwLock<bool>>,
    /// Received diagnostics
    diagnostics: Arc<RwLock<HashMap<Url, Vec<Diagnostic>>>>,
}

impl RustAnalyzerClient {
    /// Create a new rust-analyzer client
    pub async fn new(workspace_root: PathBuf) -> Result<Self> {
        // Spawn rust-analyzer process
        let mut child = Command::new("rust-analyzer")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| {
                anyhow!(
                    "Failed to spawn rust-analyzer (is it installed?): {}",
                    e
                )
            })?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("Failed to get stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("Failed to get stdout"))?;

        let client = Self {
            process: Arc::new(Mutex::new(Some(child))),
            stdin: Arc::new(Mutex::new(Some(stdin))),
            next_id: Arc::new(AtomicI32::new(1)),
            pending: Arc::new(RwLock::new(HashMap::new())),
            workspace_root,
            initialized: Arc::new(RwLock::new(false)),
            diagnostics: Arc::new(RwLock::new(HashMap::new())),
        };

        // Spawn reader task
        client.spawn_reader(stdout);

        // Initialize the server
        client.initialize().await?;

        Ok(client)
    }

    /// Spawn a task to read messages from stdout
    fn spawn_reader(&self, stdout: ChildStdout) {
        let pending = Arc::clone(&self.pending);
        let initialized = Arc::clone(&self.initialized);
        let diagnostics = Arc::clone(&self.diagnostics);

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut headers = Vec::new();
            let mut content = Vec::new();

            loop {
                // Read headers
                headers.clear();
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line).await {
                        Ok(0) => return, // EOF
                        Ok(_) => {
                            if line == "\r\n" || line == "\n" {
                                break; // End of headers
                            }
                            headers.push(line);
                        }
                        Err(e) => {
                            eprintln!("Error reading header: {}", e);
                            return;
                        }
                    }
                }

                // Parse Content-Length
                let mut content_length = 0;
                for header in &headers {
                    if let Some(len_str) = header.strip_prefix("Content-Length: ") {
                        content_length = len_str.trim().parse().unwrap_or(0);
                    }
                }

                if content_length == 0 {
                    continue;
                }

                // Read content
                content.clear();
                content.resize(content_length, 0);
                if let Err(e) = tokio::io::AsyncReadExt::read_exact(&mut reader, &mut content).await {
                    eprintln!("Error reading content: {}", e);
                    return;
                }

                // Parse message
                let msg_str = String::from_utf8_lossy(&content);
                match serde_json::from_str::<Message>(&msg_str) {
                    Ok(Message::Response(response)) => {
                        // Handle response
                        let mut pending_map = pending.write().await;
                        if let Some(pending_req) = pending_map.remove(&response.id) {
                            let result = if let Some(error) = response.error {
                                Err(anyhow!("LSP error: {}", error.message))
                            } else {
                                Ok(response.result.unwrap_or(Value::Null))
                            };
                            let _ = pending_req.tx.send(result);
                        }
                    }
                    Ok(Message::Notification(notification)) => {
                        // Handle notification
                        match notification.method.as_str() {
                            "initialized" => {
                                *initialized.write().await = true;
                            }
                            "textDocument/publishDiagnostics" => {
                                if let Some(params) = notification.params
                                    && let Ok(diag_params) = serde_json::from_value::<PublishDiagnosticsParams>(params) {
                                        diagnostics.write().await.insert(diag_params.uri, diag_params.diagnostics);
                                    }
                            }
                            _ => {
                                // Ignore other notifications
                            }
                        }
                    }
                    Ok(Message::Request(_)) => {
                        // We don't handle server requests for now
                    }
                    Err(e) => {
                        eprintln!("Failed to parse message: {} - {}", e, msg_str);
                    }
                }
            }
        });
    }

    /// Send a request and wait for response
    async fn send_request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Register pending request
        {
            let mut pending = self.pending.write().await;
            pending.insert(id, PendingRequest { tx });
        }

        // Build request
        let request = RequestMessage {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params: Some(params),
        };

        // Serialize and send
        let content = serde_json::to_string(&request)?;
        let message = format!("Content-Length: {}\r\n\r\n{}", content.len(), content);

        {
            let mut stdin = self.stdin.lock().await;
            if let Some(ref mut stdin) = *stdin {
                stdin.write_all(message.as_bytes()).await?;
                stdin.flush().await?;
            } else {
                return Err(anyhow!("stdin not available"));
            }
        }

        // Wait for response (with timeout)
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(anyhow!("Request channel closed")),
            Err(_) => Err(anyhow!("Request timed out")),
        }
    }

    /// Send a notification (no response expected)
    async fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        let notification = NotificationMessage {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: Some(params),
        };

        let content = serde_json::to_string(&notification)?;
        let message = format!("Content-Length: {}\r\n\r\n{}", content.len(), content);

        let mut stdin = self.stdin.lock().await;
        if let Some(ref mut stdin) = *stdin {
            stdin.write_all(message.as_bytes()).await?;
            stdin.flush().await?;
        }

        Ok(())
    }

    /// Initialize the LSP server
    async fn initialize(&self) -> Result<()> {
        let workspace_uri = Url::from_file_path(&self.workspace_root)
            .map_err(|_| anyhow!("Invalid workspace path"))?;

        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_path: None,
            root_uri: Some(workspace_uri.clone()),
            initialization_options: None,
            work_done_progress_params: Default::default(),
            capabilities: ClientCapabilities {
                workspace: Some(WorkspaceClientCapabilities {
                    apply_edit: Some(true),
                    workspace_edit: Some(WorkspaceEditClientCapabilities {
                        document_changes: Some(true),
                        ..Default::default()
                    }),
                    did_change_configuration: Some(DynamicRegistrationClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                        dynamic_registration: Some(false),
                        relative_pattern_support: None,
                    }),
                    symbol: Some(WorkspaceSymbolClientCapabilities {
                        dynamic_registration: Some(false),
                        ..Default::default()
                    }),
                    execute_command: Some(DynamicRegistrationClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    ..Default::default()
                }),
                text_document: Some(TextDocumentClientCapabilities {
                    synchronization: Some(TextDocumentSyncClientCapabilities {
                        dynamic_registration: Some(false),
                        will_save: Some(false),
                        will_save_wait_until: Some(false),
                        did_save: Some(false),
                    }),
                    completion: Some(CompletionClientCapabilities {
                        dynamic_registration: Some(false),
                        completion_item: Some(CompletionItemCapability {
                            snippet_support: Some(false),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    hover: Some(HoverClientCapabilities {
                        dynamic_registration: Some(false),
                        content_format: Some(vec![MarkupKind::PlainText, MarkupKind::Markdown]),
                    }),
                    definition: Some(GotoCapability {
                        dynamic_registration: Some(false),
                        link_support: Some(false),
                    }),
                    references: Some(DynamicRegistrationClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    document_symbol: Some(DocumentSymbolClientCapabilities {
                        dynamic_registration: Some(false),
                        ..Default::default()
                    }),
                    formatting: Some(DynamicRegistrationClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    rename: Some(RenameClientCapabilities {
                        dynamic_registration: Some(false),
                        ..Default::default()
                    }),
                    publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                        related_information: Some(true),
                        ..Default::default()
                    }),
                    code_action: Some(CodeActionClientCapabilities {
                        dynamic_registration: Some(false),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            trace: Some(TraceValue::Off),
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: workspace_uri,
                name: self
                    .workspace_root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
                    .to_string(),
            }]),
            client_info: Some(ClientInfo {
                name: "agent-t".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            locale: None,
        };

        let _init_result = self.send_request("initialize", serde_json::to_value(params)?).await?;

        // Send initialized notification
        self.send_notification("initialized", serde_json::json!({})).await?;

        Ok(())
    }

    /// Get diagnostics for all files
    pub async fn get_diagnostics(&self) -> HashMap<Url, Vec<Diagnostic>> {
        self.diagnostics.read().await.clone()
    }

    /// Open a document
    pub async fn did_open(&self, uri: Url, language_id: String, version: i32, text: String) -> Result<()> {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id,
                version,
                text,
            },
        };

        self.send_notification("textDocument/didOpen", serde_json::to_value(params)?).await
    }

    /// Close a document
    pub async fn did_close(&self, uri: Url) -> Result<()> {
        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri },
        };

        self.send_notification("textDocument/didClose", serde_json::to_value(params)?).await
    }

    /// Go to definition
    pub async fn goto_definition(&self, uri: Url, position: Position) -> Result<Option<Vec<Location>>> {
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let result = self.send_request("textDocument/definition", serde_json::to_value(params)?).await?;

        if result.is_null() {
            return Ok(None);
        }

        // Handle both Location and Vec<Location>
        if let Ok(location) = serde_json::from_value::<Location>(result.clone()) {
            Ok(Some(vec![location]))
        } else if let Ok(locations) = serde_json::from_value::<Vec<Location>>(result) {
            Ok(Some(locations))
        } else {
            Ok(None)
        }
    }

    /// Find references
    pub async fn find_references(&self, uri: Url, position: Position, include_declaration: bool) -> Result<Option<Vec<Location>>> {
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext {
                include_declaration,
            },
        };

        let result = self.send_request("textDocument/references", serde_json::to_value(params)?).await?;

        if result.is_null() {
            return Ok(None);
        }

        Ok(serde_json::from_value(result)?)
    }

    /// Get hover information
    pub async fn hover(&self, uri: Url, position: Position) -> Result<Option<Hover>> {
        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };

        let result = self.send_request("textDocument/hover", serde_json::to_value(params)?).await?;

        if result.is_null() {
            return Ok(None);
        }

        Ok(serde_json::from_value(result)?)
    }

    /// Get document symbols
    pub async fn document_symbols(&self, uri: Url) -> Result<Option<Vec<DocumentSymbol>>> {
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let result = self.send_request("textDocument/documentSymbol", serde_json::to_value(params)?).await?;

        if result.is_null() {
            return Ok(None);
        }

        // Try to parse as DocumentSymbol array (hierarchical)
        if let Ok(symbols) = serde_json::from_value::<Vec<DocumentSymbol>>(result.clone()) {
            return Ok(Some(symbols));
        }

        // Fall back to SymbolInformation array (flat) and convert
        if let Ok(symbol_info) = serde_json::from_value::<Vec<SymbolInformation>>(result) {
            // Convert SymbolInformation to simplified DocumentSymbol
            let symbols = symbol_info
                .into_iter()
                .map(|info| DocumentSymbol {
                    name: info.name,
                    detail: None,
                    kind: info.kind,
                    tags: info.tags,
                    deprecated: None, // Don't use deprecated field, tags should be used instead
                    range: info.location.range,
                    selection_range: info.location.range,
                    children: None,
                })
                .collect();
            return Ok(Some(symbols));
        }

        Ok(None)
    }

    /// Get workspace symbols
    pub async fn workspace_symbols(&self, query: String) -> Result<Option<Vec<SymbolInformation>>> {
        let params = WorkspaceSymbolParams {
            query,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let result = self.send_request("workspace/symbol", serde_json::to_value(params)?).await?;

        if result.is_null() {
            return Ok(None);
        }

        Ok(serde_json::from_value(result)?)
    }

    /// Get code actions
    pub async fn code_actions(&self, uri: Url, range: Range, diagnostics: Vec<Diagnostic>) -> Result<Option<Vec<CodeActionOrCommand>>> {
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range,
            context: CodeActionContext {
                diagnostics,
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let result = self.send_request("textDocument/codeAction", serde_json::to_value(params)?).await?;

        if result.is_null() {
            return Ok(None);
        }

        Ok(serde_json::from_value(result)?)
    }

    /// Get completion items
    pub async fn completion(&self, uri: Url, position: Position) -> Result<Option<Vec<CompletionItem>>> {
        let params = CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        };

        let result = self.send_request("textDocument/completion", serde_json::to_value(params)?).await?;

        if result.is_null() {
            return Ok(None);
        }

        // Handle both CompletionList and Vec<CompletionItem>
        if let Ok(completion_list) = serde_json::from_value::<CompletionList>(result.clone()) {
            Ok(Some(completion_list.items))
        } else if let Ok(items) = serde_json::from_value::<Vec<CompletionItem>>(result) {
            Ok(Some(items))
        } else {
            Ok(None)
        }
    }

    /// Rename a symbol
    pub async fn rename(&self, uri: Url, position: Position, new_name: String) -> Result<Option<WorkspaceEdit>> {
        let params = RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            new_name,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };

        let result = self.send_request("textDocument/rename", serde_json::to_value(params)?).await?;

        if result.is_null() {
            return Ok(None);
        }

        Ok(serde_json::from_value(result)?)
    }

    /// Format document
    pub async fn format(&self, uri: Url) -> Result<Option<Vec<TextEdit>>> {
        let params = DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri },
            options: FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };

        let result = self.send_request("textDocument/formatting", serde_json::to_value(params)?).await?;

        if result.is_null() {
            return Ok(None);
        }

        Ok(serde_json::from_value(result)?)
    }

    /// Shutdown the server
    pub async fn shutdown(&self) -> Result<()> {
        // Send shutdown request
        let _ = self.send_request("shutdown", Value::Null).await;

        // Send exit notification
        self.send_notification("exit", Value::Null).await?;

        // Kill the process
        let mut process = self.process.lock().await;
        if let Some(mut child) = process.take() {
            let _ = child.kill().await;
        }

        Ok(())
    }
}

// Note: Drop is not implemented because:
// 1. The process field is wrapped in Arc<Mutex<Option<Child>>>
// 2. Drop cannot be async, so we can't properly shutdown the LSP server
// 3. The process will be killed when the last Arc reference is dropped
// Users should call shutdown() explicitly for graceful termination
