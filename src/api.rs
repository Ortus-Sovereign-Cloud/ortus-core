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

use rust_web_server::extract::{FromRequest, RequestId};
use rust_web_server::prelude::*;
use rust_web_server::response::StatusCodeReasonPhrase;
use serde_json::{json, Map, Value};

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

/// A handler of the API.
pub type Handler = fn(&Request, &PathParams, &ConnectionInfo, &()) -> Response;

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
        404 => STATUS_CODE_REASON_PHRASE.n404_not_found,
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

fn health(_: &Request, _: &PathParams, _: &ConnectionInfo, _: &()) -> Response {
    ok(&json!({"status": "ok", "version": crate::VERSION}))
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

fn apps(_: &Request, _: &PathParams, _: &ConnectionInfo, _: &()) -> Response {
    // No app exists yet; each one registers here when it is added.
    ok(&json!({"apps": []}))
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

fn unknown(request: &Request, _: &PathParams, _: &ConnectionInfo, _: &()) -> Response {
    ApiError::not_found("There is no such API route.").response(request)
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

/// Registers `routes` on `app` under the prefix, and answers every other
/// path under the prefix with a `not_found` error in the API's format.
pub fn register(
    mut app: rust_web_server::state::AppWithState<()>,
    routes: &[Route],
) -> rust_web_server::state::AppWithState<()> {
    for route in routes {
        let path = format!("{PREFIX}{}", route.path);
        app = match route.method {
            Method::Get => app.get(&path, route.handler),
            Method::Post => app.post(&path, route.handler),
            Method::Put => app.put(&path, route.handler),
            Method::Patch => app.patch(&path, route.handler),
            Method::Delete => app.delete(&path, route.handler),
        };
    }
    let rest = format!("{PREFIX}/*rest");
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
