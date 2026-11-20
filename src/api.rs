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
    /// Who may call it.
    pub access: crate::auth::Access,
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

    pub fn sign_in() -> Self {
        Self {
            status: 401,
            code: "unauthenticated",
            message: "Sign in first.".into(),
        }
    }

    pub fn forbidden() -> Self {
        Self {
            status: 403,
            code: "forbidden",
            message: "You are not allowed to do this.".into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: 409,
            code: "conflict",
            message: message.into(),
        }
    }

    pub fn too_many(message: impl Into<String>) -> Self {
        Self {
            status: 429,
            code: "too_many_requests",
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
        410 => STATUS_CODE_REASON_PHRASE.n410_gone,
        413 => STATUS_CODE_REASON_PHRASE.n413_payload_too_large,
        422 => STATUS_CODE_REASON_PHRASE.n422_unprocessable_entity,
        429 => STATUS_CODE_REASON_PHRASE.n429_too_many_requests,
        _ => STATUS_CODE_REASON_PHRASE.n500_internal_server_error,
    }
}

/// The JSON body of a request, read as `T`.
pub fn body<T: serde::de::DeserializeOwned>(request: &Request) -> Result<T, ApiError> {
    serde_json::from_slice(request.body.as_bytes()).map_err(|e| {
        ApiError::bad_request(format!(
            "The request body is not what this route takes: {e}"
        ))
    })
}

/// Adds headers (such as `Set-Cookie`) to a response.
pub fn with_headers(
    mut response: Response,
    headers: Vec<rust_web_server::header::Header>,
) -> Response {
    response.headers.extend(headers);
    response
}

/// The response for an error result, or the result's own response.
pub fn answer(request: &Request, result: Result<Response, ApiError>) -> Response {
    result.unwrap_or_else(|e| e.response(request))
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

fn admin_sections(
    _: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let mut sections = Vec::new();
        for app in &ortus.apps {
            let id = app.info().id;
            if ortus.switches.is_on(id).await.unwrap_or(false) {
                for s in app.admin_sections() {
                    sections.push(
                        json!({"app": id, "path": s.path, "element": s.element, "label": s.label}),
                    );
                }
            }
        }
        ok(&json!({ "sections": sections }))
    })
}

fn sections_schema() -> Value {
    json!({"type": "object", "required": ["sections"], "properties": {"sections": {"type": "array", "description": "Sections apps add to the admin console: app, path (under /admin/), element, label (a key of the app's texts)."}}})
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

fn user_json(user: &crate::auth::accounts::User) -> Value {
    json!({
        "id": user.id, "email": user.email, "name": user.name, "role": user.role.name(),
        "language": user.language, "last_app": user.last_app, "chooser_seen": user.chooser_seen,
        "expires_at": user.expires_at,
    })
}

fn user_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id", "email", "name", "role", "chooser_seen"],
        "properties": {
            "id": {"type": "string"},
            "email": {"type": "string"},
            "name": {"type": "string"},
            "role": {"type": "string", "enum": ["member", "admin", "owner"]},
            "language": {"type": ["string", "null"], "description": "The language chosen in personal settings."},
            "last_app": {"type": ["string", "null"], "description": "The app opened last; the client opens it after sign-in."},
            "chooser_seen": {"type": "boolean", "description": "Whether the person has seen the app chooser once."},
            "expires_at": {"type": ["integer", "null"], "description": "When a demo account is deleted, in Unix milliseconds."}
        }
    })
}

fn me(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        match crate::auth::current(&request, &ortus).await {
            Some(session) => ok(&user_json(&session.user)),
            None => ApiError::sign_in().response(&request),
        }
    })
}

fn session_state(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let user = crate::auth::current(&request, &ortus)
            .await
            .map(|s| user_json(&s.user));
        ok(&json!({ "user": user }))
    })
}

fn session_schema() -> Value {
    json!({"type": "object", "required": ["user"], "properties": {"user": {"description": "The signed-in account, or null.", "oneOf": [user_schema(), {"type": "null"}]}}})
}

#[derive(serde::Deserialize)]
struct Place {
    /// The app opened, or null for the app chooser.
    app: Option<String>,
}

/// Remembers where the person is: the chooser has been seen, and which
/// app they opened last (the place to come back to after signing in).
fn set_place(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let Some(session) = crate::auth::current(&request, &ortus).await else {
            return ApiError::sign_in().response(&request);
        };
        let place: Place = match body(&request) {
            Ok(place) => place,
            Err(e) => return e.response(&request),
        };
        use crate::auth::accounts::{set, Field};
        let store = ortus.store.as_ref();
        let result = match &place.app {
            None => set(store, &session.user.id, Field::ChooserSeen).await,
            Some(app) if ortus.app(app).is_some() => {
                set(store, &session.user.id, Field::LastApp(Some(app))).await
            }
            Some(_) => return ApiError::not_installed("that").response(&request),
        };
        match result {
            Ok(()) => ok(&json!({ "app": place.app })),
            Err(_) => ApiError::internal().response(&request),
        }
    })
}

fn place_schema() -> Value {
    json!({"type": "object", "properties": {"app": {"type": ["string", "null"]}}})
}

fn sign_out(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        if let Some(token) = crate::auth::token(&request) {
            if crate::auth::sessions::end(ortus.store.as_ref(), &token)
                .await
                .is_err()
            {
                return ApiError::internal().response(&request);
            }
        }
        with_headers(
            ok(&json!({"signed_out": true})),
            vec![crate::auth::clear_cookie(ortus.config.secure())],
        )
    })
}

fn signed_out_schema() -> Value {
    json!({"type": "object", "required": ["signed_out"], "properties": {"signed_out": {"type": "boolean"}}})
}

fn setup_state(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        match crate::auth::setup::needed(ortus.store.as_ref()).await {
            Ok(needed) => ok(&json!({ "needed": needed })),
            Err(_) => ApiError::internal().response(&request),
        }
    })
}

fn setup_state_schema() -> Value {
    json!({"type": "object", "required": ["needed"], "properties": {"needed": {"type": "boolean", "description": "Whether the installation still needs its first owner."}}})
}

#[derive(serde::Deserialize)]
struct SetupBody {
    token: String,
    email: String,
    name: String,
}

/// Starts a session for `user` (and records the sign-in), returning its
/// token.
pub async fn start_session(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
    user: &crate::auth::accounts::User,
    methods: &str,
) -> Result<String, ApiError> {
    let address = crate::auth::client_address(request, connection, &ortus.config.trusted_proxies);
    let agent = request
        .header("user-agent")
        .map(|h| h.value.to_string())
        .unwrap_or_default();
    let token = crate::auth::sessions::start(
        ortus.store.as_ref(),
        &user.id,
        methods,
        &address,
        &agent,
        crate::auth::policy::lifetime(ortus.store.as_ref()).await,
        crate::program::now_ms(),
    )
    .await
    .map_err(|_| ApiError::internal())?;
    crate::auth::audit(
        ortus,
        crate::audit::Event::new("auth.sign_in", crate::audit::Outcome::Done)
            .by(&user.id)
            .from(&address)
            .with(json!({ "methods": methods })),
    )
    .await;
    Ok(token)
}

/// Starts a session for `user` and answers with the account and the
/// session cookie (and the token, for apps that are not browsers).
pub async fn signed_in(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
    user: &crate::auth::accounts::User,
    methods: &str,
) -> Response {
    signed_in_with(request, connection, ortus, user, methods, json!({})).await
}

/// As [`signed_in`], with `extra` fields in the answer.
pub async fn signed_in_with(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
    user: &crate::auth::accounts::User,
    methods: &str,
    extra: Value,
) -> Response {
    let lifetime = crate::auth::policy::lifetime(ortus.store.as_ref()).await;
    let started = start_session(request, connection, ortus, user, methods).await;
    match started {
        Ok(token) => {
            let mut body = user_json(user);
            body["session_token"] = json!(token);
            if let Value::Object(fields) = extra {
                for (key, value) in fields {
                    body[key] = value;
                }
            }
            with_headers(
                ok(&body),
                vec![crate::auth::set_cookie(
                    &token,
                    ortus.config.secure(),
                    lifetime.max_ms / 1000,
                )],
            )
        }
        Err(_) => ApiError::internal().response(request),
    }
}

fn setup(
    request: Request,
    _: PathParams,
    connection: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let body: SetupBody = match body(&request) {
            Ok(body) => body,
            Err(e) => return e.response(&request),
        };
        use crate::auth::setup::{create_owner, SetupError};
        match create_owner(
            ortus.store.as_ref(),
            &body.token,
            &body.email,
            &body.name,
            crate::program::now_ms(),
        )
        .await
        {
            Ok(user) => {
                crate::auth::audit(
                    &ortus,
                    crate::audit::Event::new("auth.owner_created", crate::audit::Outcome::Done)
                        .by(&user.id)
                        .on(&user.id)
                        .from(&crate::auth::client_address(
                            &request,
                            &connection,
                            &ortus.config.trusted_proxies,
                        )),
                )
                .await;
                signed_in(&request, &connection, &ortus, &user, "setup").await
            }
            Err(SetupError::Done) => {
                ApiError::conflict("This installation already has its owner.").response(&request)
            }
            Err(SetupError::BadToken) => {
                crate::auth::audit(
                    &ortus,
                    crate::audit::Event::new("auth.owner_created", crate::audit::Outcome::Failed)
                        .from(&crate::auth::client_address(
                            &request,
                            &connection,
                            &ortus.config.trusted_proxies,
                        ))
                        .with(json!({"reason": "bad setup token"})),
                )
                .await;
                ApiError { status: 403, code: "bad_setup_token", message: "This setup link is not valid. Run `ortus setup-link` on the server for a new one.".into() }.response(&request)
            }
            Err(SetupError::BadEmail) => {
                ApiError::bad_request("That is not an email address.").response(&request)
            }
            Err(SetupError::BadName) => ApiError::bad_request("Give a name.").response(&request),
            Err(SetupError::Store(_)) => ApiError::internal().response(&request),
        }
    })
}

fn signed_in_schema() -> Value {
    let mut schema = user_schema();
    schema["properties"]["session_token"] = json!({"type": "string", "description": "The session token. Browsers get it as a cookie too; apps send it as `Authorization: Bearer`."});
    if let Some(required) = schema["required"].as_array_mut() {
        required.push(json!("session_token"));
    }
    schema
}

/// Every operation of the core API. Apps add their own.
pub fn routes() -> Vec<Route> {
    let mut all = core_routes();
    all.extend(crate::auth::routes::routes());
    all.extend(crate::auth::passkeys::routes());
    all.extend(crate::auth::signup::routes());
    all.extend(crate::audit::routes());
    all.extend(crate::oidc::routes::routes());
    all.extend(crate::auth::external::routes());
    all.extend(crate::auth::saml::routes());
    all.extend(crate::auth::admin_users::routes());
    all.extend(crate::auth::directory::routes());
    all.extend(crate::auth::policy::routes());
    all.extend(crate::first_run::routes());
    all.extend(crate::auth::personal::routes());
    all.extend(crate::auth::app_passwords::routes());
    all.extend(crate::policy::routes());
    all.extend(crate::retention::routes());
    all.extend(crate::search::routes());
    all.extend(crate::sovereignty::routes());
    all.extend(crate::notify::routes());
    all
}

fn core_routes() -> Vec<Route> {
    vec![
        Route {
            method: Method::Get,
            path: "/me",
            id: "getMe",
            tag: "accounts",
            summary: "The signed-in person's account.",
            access: crate::auth::Access::Signed,
            response: user_schema,
            handler: me,
        },
        Route {
            method: Method::Get,
            path: "/session",
            id: "getSession",
            tag: "accounts",
            summary: "Who is signed in on this session, if anyone (never an error).",
            access: crate::auth::Access::Public,
            response: session_schema,
            handler: session_state,
        },
        Route {
            method: Method::Get,
            path: "/setup",
            id: "getSetup",
            tag: "accounts",
            summary: "Whether the installation still needs its first owner.",
            access: crate::auth::Access::Public,
            response: setup_state_schema,
            handler: setup_state,
        },
        Route {
            method: Method::Post,
            path: "/setup",
            id: "createOwner",
            tag: "accounts",
            summary:
                "Create the first owner with the token from the setup link, and sign in as them.",
            access: crate::auth::Access::Public,
            response: signed_in_schema,
            handler: setup,
        },
        Route {
            method: Method::Put,
            path: "/me/place",
            id: "setMyPlace",
            tag: "accounts",
            summary: "Record where the person is: {app: id} for the app they opened (opened again after sign-in), or {app: null} for the app chooser.",
            access: crate::auth::Access::Signed,
            response: place_schema,
            handler: set_place,
        },
        Route {
            method: Method::Post,
            path: "/session/sign-out",
            id: "signOut",
            tag: "accounts",
            summary: "End this session and remove its cookie.",
            access: crate::auth::Access::Public,
            response: signed_out_schema,
            handler: sign_out,
        },
        Route {
            method: Method::Get,
            path: "/health",
            id: "getHealth",
            tag: "core",
            summary: "Report that the server is up, and its version.",
            access: crate::auth::Access::Public,
            response: health_schema,
            handler: health,
        },
        Route {
            method: Method::Get,
            path: "/apps",
            id: "listApps",
            tag: "core",
            summary: "List the apps that are installed and switched on.",
            access: crate::auth::Access::Signed,
            response: apps_schema,
            handler: apps,
        },
        Route {
            method: Method::Get,
            path: "/admin/sections",
            id: "listAdminSections",
            tag: "admin",
            summary: "The sections installed apps add to the admin console.",
            access: crate::auth::Access::Admin,
            response: sections_schema,
            handler: admin_sections,
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
        let (handler, access) = (route.handler, route.access);
        let guarded = move |request: Request,
                            params: PathParams,
                            connection: ConnectionInfo,
                            shared: Arc<Arc<Instance>>| async move {
            let ortus: Arc<Instance> = Arc::clone(&*shared);
            match crate::auth::check(&request, &ortus, access).await {
                Ok(_) => {}
                Err(crate::auth::Refusal::SignIn) => return ApiError::sign_in().response(&request),
                Err(crate::auth::Refusal::Forbidden) => {
                    return ApiError::forbidden().response(&request)
                }
                Err(crate::auth::Refusal::NotFromClient) => {
                    return ApiError {
                        status: 403,
                        code: "not_from_client",
                        message:
                            "Changes must come from the Ortus client (X-Requested-With: ortus)."
                                .into(),
                    }
                    .response(&request)
                }
            }
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
        let mut operation = json!({
            "operationId": route.id,
            "tags": [route.tag],
            "summary": route.summary,
            "responses": {
                "200": {"description": "Success.", "headers": {"X-Request-Id": {"$ref": "#/components/headers/RequestId"}}, "content": {"application/json": {"schema": (route.response)()}}},
                "default": error_response("Any error, in the common error format.")
            }
        });
        if route.access != crate::auth::Access::Public {
            operation["security"] = json!([{"session": []}]);
            operation["description"] = json!(match route.access {
                crate::auth::Access::Admin => "For administrators and owners.",
                crate::auth::Access::Owner => "For owners.",
                crate::auth::Access::Header => "For apps: `Authorization: Bearer <session token>`, or `Basic` with the address and an app password.",
                _ => "For anyone signed in.",
            });
        }
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
            "securitySchemes": {"session": {"type": "http", "scheme": "bearer", "description": "The session token: the session cookie in the browser, or `Authorization: Bearer <token>` from an app. Requests that change something must also send `X-Requested-With: ortus`."}},
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
