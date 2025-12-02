use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        State,
    },
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;

/// Maximum number of messages to keep in history
const MAX_HISTORY: usize = 1000;

/// Message direction
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Request,  // CLI -> LLM
    Response, // LLM -> CLI
    Tool,     // Tool execution
    System,   // System messages (info, errors)
}

/// A single traffic message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficMessage {
    pub id: u64,
    pub timestamp: DateTime<Utc>,
    pub direction: Direction,
    pub message_type: String,
    pub summary: String,
    pub content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl TrafficMessage {
    pub fn new(
        id: u64,
        direction: Direction,
        message_type: impl Into<String>,
        summary: impl Into<String>,
        content: serde_json::Value,
    ) -> Self {
        Self {
            id,
            timestamp: Utc::now(),
            direction,
            message_type: message_type.into(),
            summary: summary.into(),
            content,
            duration_ms: None,
        }
    }

    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }
}

/// Shared state for the traffic inspector
pub struct InspectorState {
    tx: broadcast::Sender<TrafficMessage>,
    history: tokio::sync::RwLock<Vec<TrafficMessage>>,
    message_counter: tokio::sync::RwLock<u64>,
}

impl InspectorState {
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(256);
        Arc::new(Self {
            tx,
            history: tokio::sync::RwLock::new(Vec::new()),
            message_counter: tokio::sync::RwLock::new(0),
        })
    }

    /// Get the next message ID
    async fn next_id(&self) -> u64 {
        let mut counter = self.message_counter.write().await;
        *counter += 1;
        *counter
    }

    /// Broadcast a message to all connected clients
    pub async fn broadcast(&self, mut message: TrafficMessage) {
        message.id = self.next_id().await;

        // Add to history
        let mut history = self.history.write().await;
        history.push(message.clone());
        if history.len() > MAX_HISTORY {
            history.remove(0);
        }
        drop(history);

        // Broadcast to WebSocket clients (ignore errors if no receivers)
        let _ = self.tx.send(message);
    }

    /// Get the message history
    pub async fn get_history(&self) -> Vec<TrafficMessage> {
        self.history.read().await.clone()
    }

    /// Subscribe to message broadcasts
    pub fn subscribe(&self) -> broadcast::Receiver<TrafficMessage> {
        self.tx.subscribe()
    }
}

/// Handle for sending traffic messages
#[derive(Clone)]
pub struct TrafficHandle {
    state: Option<Arc<InspectorState>>,
}

impl TrafficHandle {
    pub fn new(state: Option<Arc<InspectorState>>) -> Self {
        Self { state }
    }

    pub fn disabled() -> Self {
        Self { state: None }
    }

    pub fn is_enabled(&self) -> bool {
        self.state.is_some()
    }

    /// Log a request being sent to the LLM
    pub async fn log_request(&self, summary: impl Into<String>, content: serde_json::Value) {
        if let Some(state) = &self.state {
            let msg = TrafficMessage::new(0, Direction::Request, "completion_request", summary, content);
            state.broadcast(msg).await;
        }
    }

    /// Log a response received from the LLM
    pub async fn log_response(
        &self,
        summary: impl Into<String>,
        content: serde_json::Value,
        duration_ms: Option<u64>,
    ) {
        if let Some(state) = &self.state {
            let mut msg =
                TrafficMessage::new(0, Direction::Response, "completion_response", summary, content);
            if let Some(d) = duration_ms {
                msg = msg.with_duration(d);
            }
            state.broadcast(msg).await;
        }
    }

    /// Log a tool execution
    pub async fn log_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        result: &str,
        duration_ms: u64,
    ) {
        if let Some(state) = &self.state {
            let content = serde_json::json!({
                "tool": tool_name,
                "arguments": args,
                "result": result,
            });
            let msg = TrafficMessage::new(
                0,
                Direction::Tool,
                "tool_execution",
                format!("Tool: {}", tool_name),
                content,
            )
            .with_duration(duration_ms);
            state.broadcast(msg).await;
        }
    }

    /// Log a system message
    pub async fn log_system(&self, message_type: &str, summary: impl Into<String>, content: serde_json::Value) {
        if let Some(state) = &self.state {
            let msg = TrafficMessage::new(0, Direction::System, message_type, summary, content);
            state.broadcast(msg).await;
        }
    }
}

/// Start the traffic inspector web server
pub async fn start_server(state: Arc<InspectorState>, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/api/history", get(history_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    println!("Traffic inspector available at http://localhost:{}", port);

    axum::serve(listener, app).await?;
    Ok(())
}

/// Serve the HTML page
async fn index_handler() -> impl IntoResponse {
    Html(include_str!("inspector.html"))
}

/// Get message history
async fn history_handler(State(state): State<Arc<InspectorState>>) -> impl IntoResponse {
    let history = state.get_history().await;
    axum::Json(history)
}

/// Handle WebSocket connections
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<InspectorState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<InspectorState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.subscribe();

    // Send history first
    let history = state.get_history().await;
    for msg in history {
        if let Ok(json) = serde_json::to_string(&msg)
            && sender.send(WsMessage::Text(json.into())).await.is_err() {
                return;
            }
    }

    // Spawn task to send broadcast messages to this client
    let send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg)
                && sender.send(WsMessage::Text(json.into())).await.is_err() {
                    break;
                }
        }
    });

    // Keep connection alive by handling incoming messages (ping/pong)
    while let Some(Ok(msg)) = receiver.next().await {
        if matches!(msg, WsMessage::Close(_)) {
            break;
        }
    }

    send_task.abort();
}
