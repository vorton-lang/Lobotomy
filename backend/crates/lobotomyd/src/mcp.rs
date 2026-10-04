//! The MCP service that roles call (harness-adapter.md §2).
//!
//! rmcp types stay inside this module (m1-plan.md §2). Each turn gets its own URL,
//! `/mcp/{token}`, so the URL identifies the caller (data-model.md §3.1) and the service keeps
//! no MCP sessions. Stateless serving also covers clients that still use the `initialize`
//! handshake of the protocol versions before 2026-07-28: rmcp answers each of their requests on
//! its own.

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, Request},
    routing::any,
};
use rmcp::{
    RoleServer, ServerHandler,
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
    },
};
use tokio_util::sync::CancellationToken;

/// The per-turn token from the request URL. The router puts it into the HTTP request extensions
/// before rmcp parses the request.
#[derive(Debug, Clone)]
pub struct TurnToken(pub String);

/// The HTTP routes of the MCP service. rmcp calls `make_handler` once per request.
pub fn router<S>(
    make_handler: impl Fn() -> S + Send + Sync + 'static,
    shutdown: CancellationToken,
) -> Router
where
    S: ServerHandler + Send + 'static,
{
    let service = StreamableHttpService::new(
        move || Ok(make_handler()),
        Arc::new(NeverSessionManager::default()),
        config(shutdown),
    );
    Router::new().route(
        "/mcp/{token}",
        any(move |Path(token): Path<String>, mut request: Request| {
            let service = service.clone();
            async move {
                request.extensions_mut().insert(TurnToken(token));
                service.handle(request).await
            }
        }),
    )
}

fn config(shutdown: CancellationToken) -> StreamableHttpServerConfig {
    // `allowed_hosts` keeps the default: loopback names only, against DNS rebinding.
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(shutdown)
}

/// The token of the turn that sent this request. `None` when the request did not come through
/// [`router`].
pub fn turn_token(context: &RequestContext<RoleServer>) -> Option<&str> {
    let parts = context.extensions.get::<http::request::Parts>()?;
    parts.extensions.get::<TurnToken>().map(|token| token.0.as_str())
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http::{StatusCode, header};
    use rmcp::{
        handler::server::{router::tool::ToolRouter, wrapper::Parameters},
        model::{ServerCapabilities, ServerConfig},
        schemars, tool, tool_handler, tool_router,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct EchoArgs {
        text: String,
    }

    #[derive(Clone)]
    struct Echo {
        #[expect(dead_code, reason = "the tool_handler macro builds its own router")]
        tool_router: ToolRouter<Self>,
    }

    #[tool_router]
    impl Echo {
        #[tool(description = "Return the caller's token and the text")]
        fn echo(
            &self,
            Parameters(EchoArgs { text }): Parameters<EchoArgs>,
            context: RequestContext<RoleServer>,
        ) -> String {
            format!("{}:{text}", turn_token(&context).unwrap_or("none"))
        }
    }

    #[tool_handler]
    impl ServerHandler for Echo {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        }
    }

    fn echo_router() -> Router {
        router(|| Echo { tool_router: Echo::tool_router() }, CancellationToken::new())
    }

    struct Reply {
        status: StatusCode,
        content_type: String,
        session_id: Option<String>,
        body: Value,
    }

    async fn send(token: &str, method: &str, headers: &[(&str, &str)], body: Option<Value>) -> Reply {
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("/mcp/{token}"))
            .header(header::HOST, "127.0.0.1:4000")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::CONTENT_TYPE, "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
        let response = echo_router().oneshot(request.body(body).unwrap()).await.unwrap();
        let header = |name| {
            response.headers().get(name).map(|value: &http::HeaderValue| value.to_str().unwrap().to_owned())
        };
        let content_type = header(header::CONTENT_TYPE.as_str()).unwrap_or_default();
        let session_id = header("mcp-session-id");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Reply { status, content_type, session_id, body }
    }

    fn tool_text(reply: &Reply) -> &str {
        reply.body["result"]["content"][0]["text"].as_str().unwrap_or_default()
    }

    #[tokio::test]
    async fn legacy_handshake_is_served_without_a_session() {
        let init = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }
        });
        let reply = send("tok-a", "POST", &[], Some(init)).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert!(reply.content_type.starts_with("application/json"), "{}", reply.content_type);
        assert_eq!(reply.body["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(reply.session_id, None);

        let call = json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "echo", "arguments": { "text": "hi" } }
        });
        let reply = send("tok-a", "POST", &[("mcp-protocol-version", "2025-06-18")], Some(call)).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert!(reply.content_type.starts_with("application/json"), "{}", reply.content_type);
        assert_eq!(tool_text(&reply), "tok-a:hi");
    }

    #[tokio::test]
    async fn stateless_protocol_call_reads_the_token() {
        let call = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {
                "name": "echo",
                "arguments": { "text": "hi" },
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        });
        let headers = [
            ("mcp-protocol-version", "2026-07-28"),
            ("mcp-method", "tools/call"),
            ("mcp-name", "echo"),
        ];
        let reply = send("tok-b", "POST", &headers, Some(call)).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert!(reply.content_type.starts_with("application/json"), "{}", reply.content_type);
        assert_eq!(tool_text(&reply), "tok-b:hi");
    }

    #[tokio::test]
    async fn get_stream_is_not_offered() {
        let reply = send("tok-c", "GET", &[], None).await;
        assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn foreign_host_is_rejected() {
        let request = http::Request::builder()
            .method("POST")
            .uri("/mcp/tok-d")
            .header(header::HOST, "attacker.example")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let response = echo_router().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
