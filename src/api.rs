//! The public API: one versioned HTTP interface for the web client, the
//! phone apps and anyone else.
//!
//! Every route lives under [`PREFIX`] and is declared once, in a [`Route`].
//! The same declaration registers the handler and writes the API reference
//! (`api/openapi.json`), so the reference cannot drift from the code: a
//! test fails when the file is stale.
//!
//! Every response carries an `X-Request-Id` header. Every error has the
//! same JSON shape:
//!
//! ```json
//! {"error": {"code": "not_found", "message": "...", "request_id": "..."}}
//! ```

use std::sync::Arc;

use rust_web_server::async_state::AsyncAppWithState;
use rust_web_server::extract::{FromRequest, RequestId};
use rust_web_server::prelude::*;
use rust_web_server::response::StatusCodeReasonPhrase;
use serde_json::{json, Map, Value};

use crate::instance::Instance;
use crate::storage::BoxFuture;

/// Path prefix of the public API. A breaking change gets a new prefix.
pub const PREFIX: &str = "/api/v1";

/// The HTTP methods the API uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub const ALL: [Method; 5] = [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
    ];

    fn lowercase(self) -> &'static str {
        match self {
            Method::Get => "get",
            Method::Post => "post",
            Method::Put => "put",
            Method::Patch => "patch",
            Method::Delete => "delete",
        }
    }
}

/// A handler of the API. It gets the request and the running installation:
///
/// ```ignore
/// fn health(_: Request, _: PathParams, _: ConnectionInfo, ortus: Arc<Instance>) -> BoxFuture<'static, Response> {
///     Box::pin(async move { ok(&json!({"status": "ok"})) })
/// }
/// ```
pub type Handler =
    fn(Request, PathParams, ConnectionInfo, Arc<Instance>) -> BoxFuture<'static, Response>;

/// One operation of the API: where it is, what it does and what it
/// answers. Paths are written without the prefix, as `/health`.
#[derive(Clone, Copy)]
pub struct Route {
    pub method: Method,
    pub path: &'static str,
    /// Stable name of the operation in the reference, in camelCase.
    pub id: &'static str,
    /// Which part of Ortus it belongs to, for grouping in the reference.
    pub tag: &'static str,
    pub summary: &'static str,
    /// The JSON schema of the 200 response body.
    pub response: fn() -> Value,
    pub handler: Handler,
}

/// An error answer of the API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    /// Stable, machine-readable name of what went wrong, in snake_case.
    pub code: &'static str,
    /// One sentence for a person. Never contains secrets.
    pub message: String,
}

impl ApiError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: 404,
            code: "not_found",
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            code: "bad_request",
            message: message.into(),
        }
    }

    /// The app that would answer is not installed or is switched off.
    pub fn not_installed(app: &str) -> Self {
        Self {
            status: 404,
            code: "not_installed",
            message: format!("The {app} app is not installed or is switched off."),
        }
    }

    pub fn internal() -> Self {
        Self {
            status: 500,
            code: "internal",
            message: "Something went wrong on the server.".into(),
        }
    }

    /// The response for this error, tagged with the request's id.
    pub fn response(&self, request: &Request) -> Response {
        let request_id = RequestId::from_request(request)
            .map(|id| id.as_str().to_string())
            .unwrap_or_default();
        let body = json!({"error": {"code": self.code, "message": self.message, "request_id": request_id}});
        Response::json(status(self.status), body.to_string().into_bytes())
    }
}

fn status(code: u16) -> &'static StatusCodeReasonPhrase {
    match code {
        400 => STATUS_CODE_REASON_PHRASE.n400_bad_request,
        401 => STATUS_CODE_REASON_PHRASE.n401_unauthorized,
        403 => STATUS_CODE_REASON_PHRASE.n403_forbidden,
        404 => STATUS_CODE_REASON_PHRASE.n404_not_found,
        409 => STATUS_CODE_REASON_PHRASE.n409_conflict,
        429 => STATUS_CODE_REASON_PHRASE.n429_too_many_requests,
        _ => STATUS_CODE_REASON_PHRASE.n500_internal_server_error,
    }
}

/// A 200 response with a JSON body.
pub fn ok(body: &Value) -> Response {
    Response::json(
        STATUS_CODE_REASON_PHRASE.n200_ok,
        body.to_string().into_bytes(),
    )
}

fn health(
    _: Request,
    _: PathParams,
    _: ConnectionInfo,
    _: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async { ok(&json!({"status": "ok", "version": crate::VERSION})) })
}

fn health_schema() -> Value {
    json!({
        "type": "object",
        "required": ["status", "version"],
        "properties": {
            "status": {"type": "string", "enum": ["ok"]},
            "version": {"type": "string", "description": "Version of the Ortus core."}
        }
    })
}

fn apps(
    _: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let apps: Vec<Value> = ortus
            .apps_on()
            .await
            .iter()
            .map(|app| json!({"id": app.id, "group": app.group.name(), "path": app.path}))
            .collect();
        ok(&json!({ "apps": apps }))
    })
}

fn apps_schema() -> Value {
    json!({
        "type": "object",
        "required": ["apps"],
        "properties": {"apps": {
            "type": "array",
            "description": "The apps that are installed and switched on, in display order.",
            "items": {
                "type": "object",
                "required": ["id", "group", "path"],
                "properties": {
                    "id": {"type": "string", "description": "Stable id of the app, such as mail or calendar."},
                    "group": {"type": "string", "enum": ["communicate", "organise", "create", "organisation"]},
                    "path": {"type": "string", "description": "Where the app lives in the web client."}
                }
            }
        }}
    })
}

/// Every operation of the core API. Apps add their own.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            method: Method::Get,
            path: "/health",
            id: "getHealth",
            tag: "core",
            summary: "Report that the server is up, and its version.",
            response: health_schema,
            handler: health,
        },
        Route {
            method: Method::Get,
            path: "/apps",
            id: "listApps",
            tag: "core",
            summary: "List the apps that are installed and switched on.",
            response: apps_schema,
            handler: apps,
        },
    ]
}

/// Registers the core's routes and every app's on `app` under the prefix,
/// and answers every other path under the prefix with a `not_found` error
/// in the API's format. An app's routes answer `not_installed` while the
/// app is switched off.
pub fn register(
    mut app: AsyncAppWithState<Arc<Instance>>,
    apps: &[Arc<dyn crate::app::App>],
) -> AsyncAppWithState<Arc<Instance>> {
    let mut all: Vec<(Option<&'static str>, Route)> =
        routes().into_iter().map(|r| (None, r)).collect();
    for owner in apps {
        let id = owner.info().id;
        all.extend(owner.routes().into_iter().map(|r| (Some(id), r)));
    }
    for (owner, route) in all {
        let path = format!("{PREFIX}{}", route.path);
        let handler = route.handler;
        let guarded = move |request: Request,
                            params: PathParams,
                            connection: ConnectionInfo,
                            shared: Arc<Arc<Instance>>| async move {
            let ortus: Arc<Instance> = Arc::clone(&*shared);
            if let Some(id) = owner {
                if !ortus.switches.is_on(id).await.unwrap_or(false) {
                    return ApiError::not_installed(id).response(&request);
                }
            }
            handler(request, params, connection, ortus).await
        };
        app = match route.method {
            Method::Get => app.get(&path, guarded),
            Method::Post => app.post(&path, guarded),
            Method::Put => app.put(&path, guarded),
            Method::Patch => app.patch(&path, guarded),
            Method::Delete => app.delete(&path, guarded),
        };
    }
    let rest = format!("{PREFIX}/*rest");
    let unknown = |request: Request, _: PathParams, _: ConnectionInfo, _: Arc<Arc<Instance>>| async move {
        ApiError::not_found("There is no such API route.").response(&request)
    };
    app.get(&rest, unknown)
        .post(&rest, unknown)
        .put(&rest, unknown)
        .patch(&rest, unknown)
        .delete(&rest, unknown)
}

/// The API reference for `routes`, as an OpenAPI 3.1 document.
pub fn reference(routes: &[Route]) -> Value {
    let error_response = |description: &str| json!({"description": description, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}});
    let mut paths = Map::new();
    for route in routes {
        let operation = json!({
            "operationId": route.id,
            "tags": [route.tag],
            "summary": route.summary,
            "responses": {
                "200": {"description": "Success.", "headers": {"X-Request-Id": {"$ref": "#/components/headers/RequestId"}}, "content": {"application/json": {"schema": (route.response)()}}},
                "default": error_response("Any error, in the common error format.")
            }
        });
        let path = paths
            .entry(format!("{PREFIX}{}", route.path))
            .or_insert_with(|| json!({}));
        path[route.method.lowercase()] = operation;
    }
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Ortus API",
            "version": "v1",
            "description": "The public API of Ortus. The web client and the phone apps use exactly this API.",
            "license": {"name": "MIT OR Apache-2.0", "identifier": "MIT OR Apache-2.0"}
        },
        "paths": paths,
        "components": {
            "headers": {"RequestId": {"description": "Identifies this request in logs. Send your own to have it echoed back.", "schema": {"type": "string"}}},
            "schemas": {
                "Error": {
                    "type": "object",
                    "required": ["error"],
                    "properties": {"error": {
                        "type": "object",
                        "required": ["code", "message", "request_id"],
                        "properties": {
                            "code": {"type": "string", "description": "Stable, machine-readable name of what went wrong."},
                            "message": {"type": "string", "description": "One sentence for a person."},
                            "request_id": {"type": "string", "description": "The X-Request-Id of the failed request."}
                        }
                    }}
                }
            }
        }
    })
}

/// The reference as the text kept in `api/openapi.json`.
pub fn reference_text(routes: &[Route]) -> String {
    let mut text = serde_json::to_string_pretty(&reference(routes)).unwrap_or_default();
    text.push('\n');
    text
}
