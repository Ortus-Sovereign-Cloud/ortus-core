//! The API of signing in and of one's own sign-in methods.

use std::sync::Arc;

use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use super::accounts::{self, State, User};
use super::{credentials, limits, Access, Session};
use crate::api::{answer, body, ok, signed_in, ApiError, Method, Route};
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture, StoreResult};

/// How long the step between a password and its one-time code may take.
const TICKET_MS: i64 = 5 * 60 * 1000;

impl From<crate::storage::StoreError> for ApiError {
    fn from(_: crate::storage::StoreError) -> Self {
        ApiError::internal()
    }
}

fn wrong() -> ApiError {
    ApiError {
        status: 401,
        code: "wrong_credentials",
        message: "That address and password, or that code, do not match.".into(),
    }
}

async fn session(request: &Request, ortus: &Arc<Instance>) -> Result<Session, ApiError> {
    super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)
}

async fn check_limits(store: &dyn AsyncStore, keys: &[String]) -> Result<(), ApiError> {
    for key in keys {
        if let Some(until) = limits::blocked_until(store, key, now_ms()).await? {
            let seconds = (until - now_ms()).max(1000) / 1000;
            return Err(ApiError::too_many(format!(
                "Too many tries. Try again in {seconds} seconds."
            )));
        }
    }
    Ok(())
}

async fn new_ticket(store: &dyn AsyncStore, user_id: &str, methods: &str) -> StoreResult<String> {
    let token = super::random_token();
    store
        .execute(
            "INSERT INTO core_signin_tickets (token_hash, user_id, methods, expires_at) VALUES ($1, $2, $3, $4)",
            &[super::sessions::hash(&token).into(), user_id.into(), methods.into(), (now_ms() + TICKET_MS).into()],
        )
        .await?;
    Ok(token)
}

/// Takes a ticket (it works once) and returns its account and methods.
async fn take_ticket(
    store: &dyn AsyncStore,
    ticket: &str,
) -> StoreResult<Option<(String, String)>> {
    let hash = super::sessions::hash(ticket);
    let mut tx = store.begin().await?;
    let rows = tx
        .query(
            "SELECT user_id, methods, expires_at FROM core_signin_tickets WHERE token_hash = $1",
            &[hash.as_str().into()],
        )
        .await?;
    // Taken by whoever deletes it: two at once cannot both use it.
    let taken = tx
        .execute(
            "DELETE FROM core_signin_tickets WHERE token_hash = $1",
            &[hash.as_str().into()],
        )
        .await?;
    tx.execute(
        "DELETE FROM core_signin_tickets WHERE expires_at < $1",
        &[now_ms().into()],
    )
    .await?;
    tx.commit().await?;
    Ok(rows
        .first()
        .filter(|_| taken == 1)
        .filter(|r| r.int(2).unwrap_or_default() > now_ms())
        .map(|r| {
            (
                r.text(0).unwrap_or_default().to_string(),
                r.text(1).unwrap_or_default().to_string(),
            )
        }))
}

/// An account that may sign in, by address.
async fn usable(store: &dyn AsyncStore, email: &str) -> StoreResult<Option<User>> {
    Ok(accounts::by_email(store, email)
        .await?
        .filter(|u| u.state == State::Active && u.expires_at.is_none_or(|at| at > now_ms())))
}

#[derive(Deserialize)]
struct PasswordBody {
    email: String,
    password: String,
}

async fn password_sign_in(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let input: PasswordBody = body(request)?;
    let store = ortus.store.as_ref();
    if !super::policy::current(store).await?.password {
        return Err(ApiError {
            status: 403,
            code: "password_off",
            message: "Passwords are turned off here. Sign in with a passkey.".into(),
        });
    }
    let keys = limits::keys(
        &input.email,
        &crate::auth::client_address(request, connection, &ortus.config.trusted_proxies),
    );
    check_limits(store, &keys).await?;
    // Many tries on one account from many networks: each waits longer.
    let wait = limits::account_delay(store, &input.email).await?;
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    let user = usable(store, &input.email).await?;
    let matches = match &user {
        Some(user) => {
            credentials::check_password(store, &user.id, &input.password, now_ms()).await?
        }
        None => {
            credentials::waste_a_password_check(&input.password);
            false
        }
    };
    let Some(user) = user.filter(|_| matches) else {
        limits::failed(store, &keys, now_ms()).await?;
        super::audit(
            ortus,
            crate::audit::Event::new("auth.sign_in", crate::audit::Outcome::Failed)
                .from(&crate::auth::client_address(
                    request,
                    connection,
                    &ortus.config.trusted_proxies,
                ))
                .with(json!({"method": "password", "email": input.email.trim().to_lowercase()})),
        )
        .await;
        return Err(wrong());
    };
    if credentials::has_totp(store, &user.id).await? {
        // The limits are cleared only once the whole sign-in succeeds; a
        // right password gives back its own try, no more.
        limits::refund(store, &keys).await?;
        let ticket = new_ticket(store, &user.id, "password").await?;
        return Ok(ok(&json!({ "next": "code", "ticket": ticket })));
    }
    // A password alone does not sign in here. Answering anything but
    // "wrong" would tell a guesser the password is right.
    limits::failed(store, &keys, now_ms()).await?;
    Err(wrong())
}

#[derive(Deserialize)]
struct CodeBody {
    ticket: String,
    code: String,
}

async fn code_sign_in(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let input: CodeBody = body(request)?;
    let store = ortus.store.as_ref();
    let Some((user_id, methods)) = take_ticket(store, &input.ticket).await? else {
        return Err(ApiError {
            status: 401,
            code: "ticket_expired",
            message: "That took too long. Sign in again.".into(),
        });
    };
    let Some(user) = accounts::by_id(store, &user_id)
        .await?
        .filter(|u| u.state == State::Active)
    else {
        return Err(wrong());
    };
    let keys = limits::keys(
        &user.email,
        &crate::auth::client_address(request, connection, &ortus.config.trusted_proxies),
    );
    check_limits(store, &keys).await?;
    if !credentials::check_totp(store, &ortus.keys, &user.id, &input.code, now_ms()).await? {
        limits::failed(store, &keys, now_ms()).await?;
        super::audit(
            ortus,
            crate::audit::Event::new("auth.sign_in", crate::audit::Outcome::Failed)
                .by(&user.id)
                .from(&crate::auth::client_address(
                    request,
                    connection,
                    &ortus.config.trusted_proxies,
                ))
                .with(json!({"method": "totp"})),
        )
        .await;
        // The ticket is used up; a new try starts with the password.
        return Err(wrong());
    }
    // The whole sign-in worked: forget the failures of the address too.
    limits::account_ok(store, &user.email).await?;
    limits::succeeded(
        store,
        &limits::keys(
            &user.email,
            &crate::auth::client_address(request, connection, &ortus.config.trusted_proxies),
        ),
    )
    .await?;
    Ok(signed_in(
        request,
        connection,
        ortus,
        &user,
        &format!("{methods}+totp"),
    )
    .await)
}

#[derive(Deserialize)]
struct RecoveryBody {
    email: String,
    code: String,
}

async fn recovery_sign_in(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let input: RecoveryBody = body(request)?;
    let store = ortus.store.as_ref();
    let keys = limits::keys(
        &input.email,
        &crate::auth::client_address(request, connection, &ortus.config.trusted_proxies),
    );
    check_limits(store, &keys).await?;
    let user = usable(store, &input.email).await?;
    let left = match &user {
        Some(user) => {
            credentials::use_recovery_code(store, &user.id, &input.code, now_ms()).await?
        }
        None => None,
    };
    let (Some(user), Some(left)) = (user, left) else {
        limits::failed(store, &keys, now_ms()).await?;
        super::audit(
            ortus,
            crate::audit::Event::new("auth.sign_in", crate::audit::Outcome::Failed)
                .from(&crate::auth::client_address(
                    request,
                    connection,
                    &ortus.config.trusted_proxies,
                ))
                .with(json!({"method": "recovery", "email": input.email.trim().to_lowercase()})),
        )
        .await;
        return Err(wrong());
    };
    limits::succeeded(store, &keys).await?;
    Ok(crate::api::signed_in_with(
        request,
        connection,
        ortus,
        &user,
        "recovery",
        json!({ "recovery_codes_left": left }),
    )
    .await)
}

async fn security(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = session(request, ortus).await?;
    let store = ortus.store.as_ref();
    let all = credentials::all(store, &me.user.id).await?;
    let has = |kind: &str| all.iter().any(|c| c.kind == kind);
    let passkeys: Vec<Value> = all
        .iter()
        .filter(|c| c.kind == "passkey")
        .map(|c| json!({"id": c.id, "name": c.name, "created_at": c.created_at, "last_used_at": c.last_used_at}))
        .collect();
    Ok(ok(&json!({
        "password": has("password"),
        "totp": has("totp"),
        "recovery_codes_left": credentials::recovery_codes_left(store, &me.user.id).await?,
        "passkeys": passkeys,
        "session_methods": me.methods,
    })))
}

#[derive(Deserialize)]
struct NewPassword {
    password: String,
    current: Option<String>,
}

async fn set_password(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = session(request, ortus).await?;
    let input: NewPassword = body(request)?;
    let store = ortus.store.as_ref();
    if !credentials::of_kind(store, &me.user.id, &credentials::Kind::Password)
        .await?
        .is_empty()
    {
        let current = input.current.unwrap_or_default();
        if !credentials::check_password(store, &me.user.id, &current, now_ms()).await? {
            return Err(ApiError {
                status: 403,
                code: "wrong_current_password",
                message: "The current password is not right.".into(),
            });
        }
    }
    if let Some(problem) = credentials::password_problem(&input.password) {
        return Err(ApiError {
            status: 400,
            code: if problem == "too_short" {
                "password_too_short"
            } else {
                "password_too_long"
            },
            message: format!(
                "A password needs at least {} characters.",
                credentials::MIN_PASSWORD
            ),
        });
    }
    credentials::set_password(store, &me.user.id, &input.password, now_ms()).await?;
    super::audit(
        ortus,
        crate::audit::Event::new("auth.credential", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .on(&me.user.id)
            .with(json!({"change": "password_set"})),
    )
    .await;
    Ok(ok(&json!({ "password": true })))
}

async fn start_totp(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = session(request, ortus).await?;
    let issuer = ortus
        .config
        .public_url
        .split("://")
        .nth(1)
        .unwrap_or("Ortus")
        .split(['/', ':'])
        .next()
        .unwrap_or("Ortus")
        .to_string();
    let (secret, uri) = credentials::start_totp(
        ortus.store.as_ref(),
        &ortus.keys,
        &me.user.id,
        &me.user.email,
        &issuer,
        now_ms(),
    )
    .await?;
    let qr = qrcode::QrCode::new(uri.as_bytes())
        .map(|code| {
            code.render::<qrcode::render::svg::Color>()
                .min_dimensions(200, 200)
                .dark_color(qrcode::render::svg::Color("#0a0b10"))
                .light_color(qrcode::render::svg::Color("#eceef3"))
                .build()
        })
        .unwrap_or_default();
    Ok(ok(&json!({ "secret": secret, "uri": uri, "qr_svg": qr })))
}

#[derive(Deserialize)]
struct ConfirmBody {
    code: String,
}

async fn confirm_totp(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = session(request, ortus).await?;
    let input: ConfirmBody = body(request)?;
    if credentials::confirm_totp(
        ortus.store.as_ref(),
        &ortus.keys,
        &me.user.id,
        &input.code,
        now_ms(),
    )
    .await?
    {
        super::audit(
            ortus,
            crate::audit::Event::new("auth.credential", crate::audit::Outcome::Done)
                .by(&me.user.id)
                .on(&me.user.id)
                .with(json!({"change": "authenticator_added"})),
        )
        .await;
        Ok(ok(&json!({ "totp": true })))
    } else {
        Err(ApiError {
            status: 400,
            code: "wrong_code",
            message:
                "That code does not match. Check the time on your phone and try the next code."
                    .into(),
        })
    }
}

async fn remove_totp(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = session(request, ortus).await?;
    super::step_up::require(ortus.store.as_ref(), &me).await?;
    let store = ortus.store.as_ref();
    for credential in credentials::of_kind(store, &me.user.id, &credentials::Kind::Totp).await? {
        credentials::remove(store, &me.user.id, &credential.id).await?;
    }
    super::audit(
        ortus,
        crate::audit::Event::new("auth.credential", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .on(&me.user.id)
            .with(json!({"change": "authenticator_removed"})),
    )
    .await;
    Ok(ok(&json!({ "totp": false })))
}

async fn new_kit(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = session(request, ortus).await?;
    super::step_up::require(ortus.store.as_ref(), &me).await?;
    let codes = credentials::new_recovery_kit(ortus.store.as_ref(), &me.user.id, now_ms()).await?;
    super::audit(
        ortus,
        crate::audit::Event::new("auth.credential", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .on(&me.user.id)
            .with(json!({"change": "recovery_kit_made"})),
    )
    .await;
    Ok(ok(&json!({ "codes": codes })))
}

macro_rules! handler {
    ($name:ident, $inner:ident, connection) => {
        fn $name(
            request: Request,
            _: PathParams,
            connection: ConnectionInfo,
            ortus: Arc<Instance>,
        ) -> BoxFuture<'static, Response> {
            Box::pin(async move {
                let result = $inner(&request, &connection, &ortus).await;
                answer(&request, result)
            })
        }
    };
    ($name:ident, $inner:ident) => {
        fn $name(
            request: Request,
            _: PathParams,
            _: ConnectionInfo,
            ortus: Arc<Instance>,
        ) -> BoxFuture<'static, Response> {
            Box::pin(async move {
                let result = $inner(&request, &ortus).await;
                answer(&request, result)
            })
        }
    };
}

handler!(h_password, password_sign_in, connection);
handler!(h_code, code_sign_in, connection);
handler!(h_recovery, recovery_sign_in, connection);
handler!(h_security, security);
handler!(h_set_password, set_password);
handler!(h_start_totp, start_totp);
handler!(h_confirm_totp, confirm_totp);
handler!(h_remove_totp, remove_totp);
handler!(h_new_kit, new_kit);

fn any_object() -> Value {
    json!({"type": "object"})
}

fn route(
    method: Method,
    path: &'static str,
    id: &'static str,
    summary: &'static str,
    access: Access,
    handler: crate::api::Handler,
) -> Route {
    Route {
        method,
        path,
        id,
        tag: "accounts",
        summary,
        access,
        response: any_object,
        handler,
    }
}

/// The sign-in routes of the core.
pub fn routes() -> Vec<Route> {
    vec![
        route(Method::Post, "/session/password", "signInWithPassword", "Sign in with an address and password. When the account has an authenticator app, answers {next: \"code\", ticket} for the one-time code.", Access::Public, h_password),
        route(Method::Post, "/session/code", "signInWithCode", "Finish signing in with the one-time code and the ticket from the password step.", Access::Public, h_code),
        route(Method::Post, "/session/recovery", "signInWithRecoveryCode", "Sign in with a recovery code. Each code works once.", Access::Public, h_recovery),
        route(Method::Get, "/me/security", "getMySecurity", "How the signed-in person can sign in: password, authenticator app, recovery codes left, passkeys.", Access::Signed, h_security),
        route(Method::Put, "/me/password", "setMyPassword", "Set or change one's password (the current one is needed to change it).", Access::Signed, h_set_password),
        route(Method::Post, "/me/totp", "startMyAuthenticator", "Start adding an authenticator app: a new secret and its otpauth address.", Access::Signed, h_start_totp),
        route(Method::Post, "/me/totp/confirm", "confirmMyAuthenticator", "Finish adding the authenticator app with its first code.", Access::Signed, h_confirm_totp),
        route(Method::Delete, "/me/totp", "removeMyAuthenticator", "Remove the authenticator app.", Access::Signed, h_remove_totp),
        route(Method::Post, "/me/recovery-kit", "newMyRecoveryKit", "Make a new recovery kit; the old codes stop working. The codes are shown only in this answer.", Access::Signed, h_new_kit),
    ]
}
