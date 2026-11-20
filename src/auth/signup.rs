//! Open sign-up, for a public demo: anyone can make an account after
//! proving their address with a code sent there. Sign-up is limited per
//! day and per network address, and its accounts are deleted with all
//! their data after `signup.account_days`.
//!
//! The form never says whether an address already has an account: such an
//! address gets a message saying so instead of a code.

use std::sync::Arc;

use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::accounts::{self, NewUser, Role, State};
use super::{mail_text, Access};
use crate::api::{answer, body, ok, signed_in_with, ApiError, Method, Route};
use crate::app::JobKind;
use crate::instance::Instance;
use crate::mail_out::{self, Outgoing};
use crate::program::now_ms;
use crate::storage::{AsyncStore, AsyncTx, BoxFuture, StoreResult};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const CODE_MS: i64 = 15 * 60 * 1000;
const CODE_TRIES: i64 = 5;

/// The hourly job that deletes expired accounts.
pub const EXPIRY_JOB: &str = "core.expire-accounts";

fn hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn six_digits() -> String {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).expect("the operating system's random numbers");
    format!("{:06}", u32::from_be_bytes(bytes) % 1_000_000)
}

fn closed() -> ApiError {
    ApiError::not_found("Sign-up is not open on this installation.")
}

/// Sign-up mails (codes and "already signed up" notices) per address a day.
const MAILS_PER_EMAIL: i64 = 5;
/// Sign-up mails sent for requests from one network a day.
const MAILS_PER_ADDRESS: i64 = 30;

async fn count_since(
    store: &dyn AsyncStore,
    since: i64,
    address: Option<&str>,
) -> StoreResult<i64> {
    let rows = match address {
        Some(address) => {
            store
                .query(
                    "SELECT COUNT(*) FROM core_signups WHERE created_at >= $1 AND address = $2",
                    &[since.into(), address.into()],
                )
                .await?
        }
        None => {
            store
                .query(
                    "SELECT COUNT(*) FROM core_signups WHERE created_at >= $1",
                    &[since.into()],
                )
                .await?
        }
    };
    Ok(rows.first().and_then(|r| r.int(0)).unwrap_or_default())
}

#[derive(Deserialize)]
struct SignUp {
    email: String,
    name: String,
    language: Option<String>,
    accept_terms: bool,
}

async fn sign_up(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let config = &ortus.config;
    if !config.signup.open {
        return Err(closed());
    }
    // Nobody signs up before the first owner exists: setup comes first.
    if super::setup::needed(ortus.store.as_ref()).await? {
        return Err(closed());
    }
    let input: SignUp = body(request)?;
    if !input.accept_terms {
        return Err(ApiError {
            status: 400,
            code: "terms_not_accepted",
            message: "Accept the terms to sign up.".into(),
        });
    }
    let email = accounts::normal_email(&input.email).ok_or_else(|| ApiError {
        status: 400,
        code: "bad_email",
        message: "That is not an email address.".into(),
    })?;
    let name = input.name.trim();
    if name.is_empty() || name.len() > 200 {
        return Err(ApiError {
            status: 400,
            code: "bad_name",
            message: "Give your name.".into(),
        });
    }
    let store = ortus.store.as_ref();
    let now = now_ms();
    let address = crate::auth::client_address(request, connection, &ortus.config.trusted_proxies);
    if count_since(store, now - DAY_MS, None).await? >= i64::from(config.signup.accounts_per_day) {
        return Err(ApiError {
            status: 429,
            code: "signup_full",
            message: "Today's new accounts are all taken. Try again tomorrow.".into(),
        });
    }
    if count_since(store, now - DAY_MS, Some(&address)).await?
        >= i64::from(config.signup.accounts_per_address_per_day)
    {
        return Err(ApiError {
            status: 429,
            code: "signup_address_full",
            message: "Too many new accounts from this network today.".into(),
        });
    }
    // Every mail sent counts, new account or not: resends would otherwise
    // flood the address and give fresh guesses at the code without end.
    let sent = |column: &'static str, value: String| async move {
        store
            .query(
                &format!(
                    "SELECT COUNT(*) FROM core_signup_mails WHERE {column} = $1 AND sent_at >= $2"
                ),
                &[value.into(), (now - DAY_MS).into()],
            )
            .await
            .map(|rows| rows.first().and_then(|r| r.int(0)).unwrap_or_default())
    };
    if sent("email", email.clone()).await? >= MAILS_PER_EMAIL
        || sent("address", address.clone()).await? >= MAILS_PER_ADDRESS
    {
        return Err(ApiError {
            status: 429,
            code: "signup_address_full",
            message: "Too many sign-up mails today. Try again tomorrow.".into(),
        });
    }
    store
        .execute(
            "INSERT INTO core_signup_mails (email, address, sent_at) VALUES ($1, $2, $3)",
            &[email.as_str().into(), address.as_str().into(), now.into()],
        )
        .await?;
    let language = mail_text::language(input.language.as_deref());
    let ticket = super::random_token();
    match accounts::by_email(store, &email).await? {
        Some(existing) if existing.state != State::Unverified => {
            let (subject, text) = mail_text::already_signed_up(language, &config.public_url);
            mail_out::queue(
                store,
                &Outgoing {
                    to: email,
                    subject,
                    text,
                },
                now,
            )
            .await?;
            // The same answer as a new account: the ticket leads nowhere.
            return Ok(ok(&json!({ "ticket": ticket })));
        }
        existing => {
            let code = six_digits();
            let mut tx = store.begin().await?;
            let user_id = match existing {
                Some(user) => user.id,
                None => {
                    let expires = now + i64::from(config.signup.account_days) * DAY_MS;
                    let user = accounts::create_in(
                        &mut *tx,
                        &NewUser {
                            email: &email,
                            name,
                            role: Role::Member,
                            state: State::Unverified,
                            expires_at: Some(expires),
                        },
                        now,
                    )
                    .await?;
                    tx.execute(
                        "UPDATE core_users SET quota_bytes = $1, language = $2 WHERE id = $3",
                        &[
                            ((config.signup.quota_mb * 1024 * 1024) as i64).into(),
                            language.into(),
                            user.id.as_str().into(),
                        ],
                    )
                    .await?;
                    tx.execute("INSERT INTO core_signups (user_id, address, created_at) VALUES ($1, $2, $3)", &[user.id.as_str().into(), address.as_str().into(), now.into()]).await?;
                    user.id
                }
            };
            tx.execute(
                "DELETE FROM core_email_checks WHERE user_id = $1",
                &[user_id.as_str().into()],
            )
            .await?;
            tx.execute(
                "INSERT INTO core_email_checks (ticket_hash, user_id, code_hash, attempts, expires_at) VALUES ($1, $2, $3, 0, $4)",
                &[hash(&ticket).into(), user_id.as_str().into(), hash(&code).into(), (now + CODE_MS).into()],
            )
            .await?;
            tx.commit().await?;
            let (subject, text) = mail_text::signup_code(language, &code, &config.public_url);
            mail_out::queue(
                store,
                &Outgoing {
                    to: email,
                    subject,
                    text,
                },
                now,
            )
            .await?;
        }
    }
    Ok(ok(&json!({ "ticket": ticket })))
}

#[derive(Deserialize)]
struct Verify {
    ticket: String,
    code: String,
}

async fn verify(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    if !ortus.config.signup.open {
        return Err(closed());
    }
    let input: Verify = body(request)?;
    let store = ortus.store.as_ref();
    let wrong = || ApiError {
        status: 401,
        code: "wrong_code",
        message: "That code does not match, or it has expired.".into(),
    };
    let ticket = hash(&input.ticket);
    let now = now_ms();
    // Count the try before comparing, in one statement, so parallel
    // guesses cannot share one try.
    let counted = store
        .execute(
            "UPDATE core_email_checks SET attempts = attempts + 1 WHERE ticket_hash = $1 AND attempts < $2 AND expires_at > $3",
            &[ticket.as_str().into(), CODE_TRIES.into(), now.into()],
        )
        .await?;
    if counted != 1 {
        store
            .execute(
                "DELETE FROM core_email_checks WHERE ticket_hash = $1",
                &[ticket.as_str().into()],
            )
            .await?;
        return Err(wrong());
    }
    let rows = store
        .query(
            "SELECT user_id, code_hash FROM core_email_checks WHERE ticket_hash = $1",
            &[ticket.as_str().into()],
        )
        .await?;
    let Some(row) = rows.first() else {
        return Err(wrong());
    };
    let user_id = row.text(0).unwrap_or_default().to_string();
    let code: String = input.code.chars().filter(char::is_ascii_digit).collect();
    if hash(&code) != row.text(1).unwrap_or_default() {
        return Err(wrong());
    }
    // Only a sign-up still waiting for its check becomes active (not one
    // an administrator suspended meanwhile).
    let mut tx = store.begin().await?;
    tx.execute(
        "DELETE FROM core_email_checks WHERE ticket_hash = $1",
        &[ticket.as_str().into()],
    )
    .await?;
    let activated = tx
        .execute(
            "UPDATE core_users SET state = 'active' WHERE id = $1 AND state = 'unverified'",
            &[user_id.as_str().into()],
        )
        .await?;
    tx.commit().await?;
    if activated != 1 {
        return Err(wrong());
    }
    super::audit(
        ortus,
        crate::audit::Event::new("auth.sign_up", crate::audit::Outcome::Done)
            .by(&user_id)
            .on(&user_id)
            .from(&crate::auth::client_address(
                request,
                connection,
                &ortus.config.trusted_proxies,
            )),
    )
    .await;
    let user = accounts::by_id(store, &user_id)
        .await?
        .ok_or_else(ApiError::internal)?;
    // The account has no way to sign in yet: the page goes on to add one.
    Ok(signed_in_with(
        request,
        connection,
        ortus,
        &user,
        "email",
        json!({ "next": "add_sign_in_method" }),
    )
    .await)
}

async fn site(_: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let config = &ortus.config;
    Ok(ok(&json!({
        "website": config.website,
        "signup_open": config.signup.open,
        "account_days": config.signup.open.then_some(config.signup.account_days),
        "quota_mb": config.signup.open.then_some(config.signup.quota_mb),
    })))
}

/// Deletes accounts whose time is up (and sign-ups never verified within
/// a day), with all their data, a batch at a time.
async fn expire(tx: &mut dyn AsyncTx, ortus: &Instance, now: i64) -> Result<(), String> {
    // Counting rows are needed for a day only; dead tokens not at all.
    for (sql, before) in [
        (
            "DELETE FROM core_signup_mails WHERE sent_at < $1",
            now - DAY_MS,
        ),
        (
            "DELETE FROM core_signups WHERE created_at < $1",
            now - 2 * DAY_MS,
        ),
        ("DELETE FROM core_oidc_tokens WHERE expires_at < $1", now),
    ] {
        tx.execute(sql, &[before.into()])
            .await
            .map_err(|e| e.to_string())?;
    }
    let rows = tx
        .query(
            &format!("SELECT id FROM core_users WHERE ((expires_at IS NOT NULL AND expires_at <= $1) OR (state = 'unverified' AND created_at <= $2)) AND id NOT IN ({}) ORDER BY id LIMIT 100", crate::retention::PERSON_HELD_SQL),
            &[now.into(), (now - DAY_MS).into()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let rules = crate::retention::read_rules(tx, now)
        .await
        .map_err(|e| e.to_string())?;
    for row in rows {
        let id = row.text(0).unwrap_or_default().to_string();
        for app in &ortus.apps {
            app.forget_user(tx, &id, &rules).await?;
        }
        crate::search::forget_in(tx, ortus.store.engine(), &id)
            .await
            .map_err(|e| e.to_string())?;
        for sql in [
            "DELETE FROM core_sessions WHERE user_id = $1",
            "DELETE FROM core_credentials WHERE user_id = $1",
            "DELETE FROM core_signin_tickets WHERE user_id = $1",
            "DELETE FROM core_webauthn_challenges WHERE user_id = $1",
            "DELETE FROM core_email_checks WHERE user_id = $1",
            "DELETE FROM core_group_members WHERE user_id = $1",
            "DELETE FROM core_invites WHERE user_id = $1",
            "DELETE FROM core_external_links WHERE user_id = $1",
            "DELETE FROM core_preferences WHERE user_id = $1",
            "DELETE FROM core_oidc_codes WHERE user_id = $1",
            "DELETE FROM core_oidc_tokens WHERE user_id = $1",
            "DELETE FROM core_signups WHERE user_id = $1",
            "DELETE FROM core_users WHERE id = $1",
        ] {
            tx.execute(sql, &[id.as_str().into()])
                .await
                .map_err(|e| e.to_string())?;
        }
        crate::audit::record_in(
            tx,
            &crate::audit::Event::new("auth.account_deleted", crate::audit::Outcome::Done)
                .on(&id)
                .with(json!({"reason": "expired"})),
            now,
        )
        .await
        .map_err(|e| e.to_string())?;
        println!("ortus: deleted expired account {id} and its data");
    }
    Ok(())
}

/// The job behind [`EXPIRY_JOB`].
pub fn expiry_job(ortus: Arc<Instance>) -> JobKind {
    JobKind {
        kind: EXPIRY_JOB.into(),
        handler: Box::new(move |_job, tx| {
            let ortus = Arc::clone(&ortus);
            Box::pin(async move { expire(tx, &ortus, now_ms()).await })
        }),
    }
}

fn h_sign_up(
    request: Request,
    _: PathParams,
    connection: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, sign_up(&request, &connection, &ortus).await) })
}
fn h_verify(
    request: Request,
    _: PathParams,
    connection: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, verify(&request, &connection, &ortus).await) })
}
fn h_site(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, site(&request, &ortus).await) })
}

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    let route = |method, path, id, summary, access, handler| Route {
        method,
        path,
        id,
        tag: "accounts",
        summary,
        access,
        response: any_object,
        handler,
    };
    vec![
        route(Method::Get, "/site", "getSite", "What this installation offers before sign-in: the project website, open sign-up and its limits.", Access::Public, h_site as crate::api::Handler),
        route(Method::Post, "/signup", "signUp", "Start an account with {email, name, language, accept_terms}; a code goes to the address. Answers {ticket}.", Access::Public, h_sign_up),
        route(Method::Post, "/signup/verify", "verifySignUp", "Finish signing up with {ticket, code}; signs in and answers next: add_sign_in_method.", Access::Public, h_verify),
    ]
}
