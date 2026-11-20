//! App passwords: generated passwords, one per mail, calendar or contacts
//! app, for the protocols that cannot do passkeys (IMAP, SMTP submission,
//! CalDAV, CardDAV). Each is shown once, kept as an Argon2 hash, can be
//! removed on its own, and is checked with the same attempt limits as
//! signing in. They never sign in to the web.

use std::sync::Arc;

use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use super::accounts::{self, State, User};
use super::credentials::{self, Kind};
use super::{limits, Access};
use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture};

/// Letters only, none easily confused (no i, l, o).
const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz";

/// A new app password: four groups of four letters (about 72 bits).
pub fn generate() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the operating system's random numbers");
    let letters: Vec<char> = bytes
        .iter()
        .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
        .collect();
    letters
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

/// Why a protocol sign-in was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Too many tries: wait until then (Unix ms).
    Blocked(i64),
    Wrong,
}

/// Checks an address and app password, as IMAP or SMTP would; `client`
/// is the network address for the attempt limits.
pub async fn authenticate(
    instance: &Instance,
    email: &str,
    password: &str,
    client: &str,
) -> Result<User, Refused> {
    let store = instance.store.as_ref();
    let keys = limits::keys(email, client);
    for key in &keys {
        if let Ok(Some(until)) = limits::blocked_until(store, key, now_ms()).await {
            return Err(Refused::Blocked(until));
        }
    }
    // Spaces and dashes as people type them; letters only otherwise.
    let typed: String = password
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let user = accounts::by_email(store, email)
        .await
        .ok()
        .flatten()
        .filter(|u| u.state == State::Active);
    let mut found = None;
    match &user {
        Some(user) => {
            for credential in credentials::of_kind(store, &user.id, &Kind::AppPassword)
                .await
                .unwrap_or_default()
            {
                if credentials::password_matches(
                    &typed,
                    credential.data["hash"].as_str().unwrap_or_default(),
                ) {
                    found = Some(credential);
                    break;
                }
            }
        }
        None => credentials::waste_a_password_check(&typed),
    }
    let (Some(user), Some(credential)) = (user, found) else {
        super::audit(
            instance,
            crate::audit::Event::new("auth.sign_in", crate::audit::Outcome::Failed)
                .from(client)
                .with(json!({ "method": "app_password", "email": email.trim().to_lowercase() })),
        )
        .await;
        return Err(Refused::Wrong);
    };
    let _ = limits::succeeded(store, &keys).await;
    let _ = credentials::update(store, &credential.id, &credential.data, now_ms()).await;
    Ok(user)
}

async fn list(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let listed: Vec<Value> = credentials::of_kind(ortus.store.as_ref(), &me.user.id, &Kind::AppPassword)
        .await?
        .iter()
        .map(|c| json!({ "id": c.id, "name": c.name, "created_at": c.created_at, "last_used_at": c.last_used_at }))
        .collect();
    Ok(ok(&json!({ "app_passwords": listed })))
}

#[derive(Deserialize)]
struct New {
    name: String,
}

async fn create(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let input: New = body(request)?;
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::bad_request(
            "Name the app (up to 100 characters).",
        ));
    }
    let store: &dyn AsyncStore = ortus.store.as_ref();
    if credentials::of_kind(store, &me.user.id, &Kind::AppPassword)
        .await?
        .len()
        >= 20
    {
        return Err(ApiError {
            status: 409,
            code: "too_many",
            message: "Twenty app passwords at most; remove one first.".into(),
        });
    }
    let password = generate();
    let plain: String = password.chars().filter(char::is_ascii_alphabetic).collect();
    let id = credentials::put(
        store,
        &me.user.id,
        &Kind::AppPassword,
        name,
        &json!({ "hash": credentials::hash_password(&plain) }),
        false,
        now_ms(),
    )
    .await?;
    super::audit(
        ortus,
        crate::audit::Event::new("auth.app_password_added", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .with(json!({ "name": name })),
    )
    .await;
    // Shown this once only.
    Ok(ok(&json!({ "id": id, "name": name, "password": password })))
}

async fn remove(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let id = params.get("id").unwrap_or_default();
    let mine = credentials::of_kind(ortus.store.as_ref(), &me.user.id, &Kind::AppPassword)
        .await?
        .into_iter()
        .any(|c| c.id == id);
    if !mine || !credentials::remove(ortus.store.as_ref(), &me.user.id, id).await? {
        return Err(ApiError::not_found("There is no such app password."));
    }
    super::audit(
        ortus,
        crate::audit::Event::new("auth.app_password_removed", crate::audit::Outcome::Done)
            .by(&me.user.id),
    )
    .await;
    list(request, ortus).await
}

fn h_list(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, list(&request, &ortus).await) })
}
fn h_create(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, create(&request, &ortus).await) })
}
fn h_remove(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, remove(&request, &params, &ortus).await) })
}

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    let r = |method, path, id, summary, handler| Route {
        method,
        path,
        id,
        tag: "me",
        summary,
        access: Access::Signed,
        response: any_object,
        handler,
    };
    vec![
        r(
            Method::Get,
            "/me/app-passwords",
            "listAppPasswords",
            "One's app passwords (names and dates, never the passwords).",
            h_list as crate::api::Handler,
        ),
        r(
            Method::Post,
            "/me/app-passwords",
            "createAppPassword",
            "Make an app password for one app: {name}. The answer shows the password, this once.",
            h_create,
        ),
        r(
            Method::Delete,
            "/me/app-passwords/:id",
            "removeAppPassword",
            "Remove an app password; the app it was for can no longer sign in.",
            h_remove,
        ),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_passwords_look_right() {
        let password = super::generate();
        assert_eq!(19, password.len());
        assert_eq!(4, password.split('-').count());
        assert!(password
            .chars()
            .all(|c| c == '-' || super::ALPHABET.contains(&(c as u8))));
        assert_ne!(password, super::generate());
    }
}
