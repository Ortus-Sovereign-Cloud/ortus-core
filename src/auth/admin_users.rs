//! Administering people: the list with what an administrator needs to
//! see, invitations, roles, suspension, quotas, groups and CSV import.

use std::sync::Arc;

use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use super::accounts::{self, Field, NewUser, Role, State, User};
use super::{credentials, mail_text, sessions, Access};
use crate::api::{answer, body, ok, signed_in_with, ApiError, Method, Route};
use crate::instance::Instance;
use crate::mail_out::{self, Outgoing};
use crate::oidc::sha256_hex;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture, Migration, Schema, StoreError, StoreResult};

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "groups and invitations",
        sql: "CREATE TABLE core_groups (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        created_at BIGINT NOT NULL
    );
    CREATE TABLE core_group_members (
        group_id TEXT NOT NULL REFERENCES core_groups (id),
        user_id TEXT NOT NULL REFERENCES core_users (id),
        PRIMARY KEY (group_id, user_id)
    );
    CREATE TABLE core_invites (
        token_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES core_users (id),
        expires_at BIGINT NOT NULL
    )",
    },
    // Where the letter went (a personal address), for sending it again.
    Migration {
        version: 2,
        name: "invitation letters to another address",
        sql: "ALTER TABLE core_invites ADD COLUMN send_to TEXT",
    },
];

pub const SCHEMA: Schema = Schema {
    app: "core.people",
    migrations: MIGRATIONS,
};

/// How long an invitation link works.
const INVITE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// What an administrator sees of one person.
async fn row(store: &dyn AsyncStore, user: &User) -> StoreResult<Value> {
    let kinds: Vec<String> = credentials::all(store, &user.id)
        .await?
        .into_iter()
        .map(|c| c.kind)
        .collect();
    let second = if kinds.iter().any(|k| k == "passkey") {
        "key"
    } else if kinds.iter().any(|k| k == "totp") {
        "app"
    } else {
        "off"
    };
    let seen = store
        .query(
            "SELECT MAX(last_seen_at) FROM core_sessions WHERE user_id = $1",
            &[user.id.as_str().into()],
        )
        .await?;
    let quota = store
        .query(
            "SELECT quota_bytes FROM core_users WHERE id = $1",
            &[user.id.as_str().into()],
        )
        .await?;
    let groups = store
        .query("SELECT g.name FROM core_groups g JOIN core_group_members m ON m.group_id = g.id WHERE m.user_id = $1 ORDER BY g.name", &[user.id.as_str().into()])
        .await?;
    let invitation = store
        .query(
            "SELECT expires_at, send_to FROM core_invites WHERE user_id = $1",
            &[user.id.as_str().into()],
        )
        .await?
        .first()
        .map(|r| json!({"expires_at": r.int(0), "send_to": r.text(1)}));
    Ok(json!({
        "id": user.id, "email": user.email, "name": user.name, "role": user.role.name(), "state": user.state.name(),
        "sign_in": second,
        "last_active": seen.first().and_then(|r| r.int(0)),
        "quota_bytes": quota.first().and_then(|r| r.int(0)),
        // Storage use comes from the apps that keep files (drive, mail).
        "used_bytes": 0,
        "groups": groups.iter().filter_map(|r| r.text(0)).collect::<Vec<_>>(),
        "expires_at": user.expires_at,
        "invitation": invitation,
    }))
}

/// Everyone's listing facts in four queries, whatever the number of people.
async fn everyone(store: &dyn AsyncStore) -> StoreResult<Vec<Value>> {
    use std::collections::HashMap;
    let mut second: HashMap<String, &str> = HashMap::new();
    for r in store
        .query(
            "SELECT user_id, kind FROM core_credentials WHERE kind IN ('passkey', 'totp')",
            &[],
        )
        .await?
    {
        let id = r.text(0).unwrap_or_default().to_string();
        let kind = if r.text(1) == Some("passkey") {
            "key"
        } else {
            "app"
        };
        let entry = second.entry(id).or_insert(kind);
        if kind == "key" {
            *entry = "key";
        }
    }
    let seen: HashMap<String, i64> = store
        .query(
            "SELECT user_id, MAX(last_seen_at) FROM core_sessions GROUP BY user_id",
            &[],
        )
        .await?
        .iter()
        .filter_map(|r| Some((r.text(0)?.to_string(), r.int(1)?)))
        .collect();
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for r in store.query("SELECT m.user_id, g.name FROM core_group_members m JOIN core_groups g ON g.id = m.group_id ORDER BY g.name", &[]).await? {
        groups.entry(r.text(0).unwrap_or_default().to_string()).or_default().push(r.text(1).unwrap_or_default().to_string());
    }
    let invitations: HashMap<String, Value> = store
        .query("SELECT user_id, expires_at, send_to FROM core_invites", &[])
        .await?
        .iter()
        .filter_map(|r| {
            Some((
                r.text(0)?.to_string(),
                json!({"expires_at": r.int(1), "send_to": r.text(2)}),
            ))
        })
        .collect();
    let quotas: HashMap<String, i64> = store
        .query(
            "SELECT id, quota_bytes FROM core_users WHERE quota_bytes IS NOT NULL",
            &[],
        )
        .await?
        .iter()
        .filter_map(|r| Some((r.text(0)?.to_string(), r.int(1)?)))
        .collect();
    Ok(accounts::list(store)
        .await?
        .iter()
        .map(|user| {
            json!({
                "id": user.id, "email": user.email, "name": user.name, "role": user.role.name(), "state": user.state.name(),
                "sign_in": second.get(&user.id).copied().unwrap_or("off"),
                "last_active": seen.get(&user.id),
                "quota_bytes": quotas.get(&user.id),
                // Storage use comes from the apps that keep files (drive, mail).
                "used_bytes": 0,
                "groups": groups.get(&user.id).cloned().unwrap_or_default(),
                "expires_at": user.expires_at,
                "invitation": invitations.get(&user.id),
            })
        })
        .collect())
}

async fn list(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let store = ortus.store.as_ref();
    let uri = request.request_uri.clone();
    let q = crate::form::query(&uri, "q")
        .unwrap_or_default()
        .to_lowercase();
    let filter = crate::form::query(&uri, "filter").unwrap_or_default();
    let mut people = Vec::new();
    let (mut without_second, mut near_quota) = (0, 0);
    let all = everyone(store).await?;
    let total = all.len();
    for entry in all {
        let no_second = entry["sign_in"] == "off";
        let near = entry["quota_bytes"]
            .as_i64()
            .is_some_and(|quota| entry["used_bytes"].as_i64().unwrap_or(0) * 10 >= quota * 9);
        without_second += i64::from(no_second);
        near_quota += i64::from(near);
        let text = |field: &str| entry[field].as_str().unwrap_or_default().to_lowercase();
        let groups = entry["groups"].to_string().to_lowercase();
        if !q.is_empty()
            && !text("name").contains(&q)
            && !text("email").contains(&q)
            && !groups.contains(&q)
        {
            continue;
        }
        if (filter == "no_2fa" && !no_second) || (filter == "near_quota" && !near) {
            continue;
        }
        people.push(entry);
    }
    let groups = store.query("SELECT COUNT(*) FROM core_groups", &[]).await?;
    Ok(ok(&json!({
        "users": people,
        "total": total,
        "groups": groups.first().and_then(|r| r.int(0)).unwrap_or(0),
        "without_second_factor": without_second,
        "near_quota": near_quota,
    })))
}

async fn me_admin(request: &Request, ortus: &Arc<Instance>) -> Result<User, ApiError> {
    Ok(super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?
        .user)
}

/// A new invitation: its link (always, so an administrator can pass it
/// on), when it stops working, and where its letter went.
pub struct Sent {
    pub link: String,
    pub expires_at: i64,
    /// The address the letter went to, if one went.
    pub sent_to: Option<String>,
    /// Why no letter went: `no_mail` (this installation sends none),
    /// `own_mailbox` (the address is a mailbox here, which its person
    /// cannot read before joining) or `not_asked`.
    pub not_sent: Option<&'static str>,
}

impl Sent {
    /// The answer's fields, with the link as a QR code too.
    fn json(&self) -> Value {
        let qr = qrcode::QrCode::new(self.link.as_bytes())
            .map(|code| {
                code.render::<qrcode::render::svg::Color>()
                    .min_dimensions(200, 200)
                    .dark_color(qrcode::render::svg::Color("#0a0b10"))
                    .light_color(qrcode::render::svg::Color("#eceef3"))
                    .build()
            })
            .ok();
        json!({
            "link": self.link,
            "expires_at": self.expires_at,
            "sent_to": self.sent_to,
            "not_sent": self.not_sent,
            "qr_svg": qr,
        })
    }
}

/// Whether `email` is a mailbox in one of this installation's mail
/// domains (asked of the mail app; without one, no address is).
async fn own_mailbox(ortus: &Instance, email: &str) -> bool {
    let Some((_, domain)) = email.rsplit_once('@') else {
        return false;
    };
    match ortus.service::<dyn crate::interfaces::MailDomains>().await {
        Ok(domains) => domains.is_ours(&domain.to_ascii_lowercase()).await,
        Err(_) => false,
    }
}

/// A personal address to send the letter to, checked; none when empty or
/// the account's own.
fn personal(send_to: Option<&str>, account: &str) -> Result<Option<String>, ApiError> {
    let Some(raw) = send_to.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let address = accounts::normal_email(raw)
        .ok_or_else(|| ApiError::bad_request(format!("'{raw}' is not an email address.")))?;
    Ok((address != account).then_some(address))
}

/// Makes a new invitation link for `user` (the old one stops working) and,
/// when `send`, mails it: to `send_to`, else to the account's address
/// unless that is a mailbox here.
async fn send_invite(
    ortus: &Instance,
    user: &User,
    inviter: &str,
    language: Option<&str>,
    send_to: Option<String>,
    send: bool,
) -> StoreResult<Sent> {
    let token = super::random_token();
    let store = ortus.store.as_ref();
    let expires_at = now_ms() + INVITE_MS;
    store
        .execute(
            "DELETE FROM core_invites WHERE user_id = $1",
            &[user.id.as_str().into()],
        )
        .await?;
    store
        .execute(
            "INSERT INTO core_invites (token_hash, user_id, expires_at, send_to) VALUES ($1, $2, $3, $4)",
            &[
                sha256_hex(&token).into(),
                user.id.as_str().into(),
                expires_at.into(),
                send_to.as_deref().into(),
            ],
        )
        .await?;
    let link = format!("{}/welcome/accept#{token}", ortus.config.public_url);
    let mut sent = Sent {
        link,
        expires_at,
        sent_to: None,
        not_sent: None,
    };
    if !send {
        sent.not_sent = Some("not_asked");
        return Ok(sent);
    }
    if ortus.config.outgoing_mail.is_none() {
        sent.not_sent = Some("no_mail");
        return Ok(sent);
    }
    let to = match send_to {
        Some(address) => address,
        None if own_mailbox(ortus, &user.email).await => {
            sent.not_sent = Some("own_mailbox");
            return Ok(sent);
        }
        None => user.email.clone(),
    };
    let elsewhere = (to != user.email).then_some(user.email.as_str());
    let (subject, text) = mail_text::invitation(
        mail_text::language(language),
        inviter,
        &sent.link,
        elsewhere,
    );
    mail_out::queue(
        store,
        &Outgoing {
            to: to.clone(),
            subject,
            text,
        },
        now_ms(),
    )
    .await?;
    sent.sent_to = Some(to);
    Ok(sent)
}

#[derive(Deserialize)]
struct Invite {
    email: String,
    name: String,
    role: Option<String>,
    groups: Option<Vec<String>>,
    language: Option<String>,
    /// A personal address for the letter (the account keeps `email`).
    send_to: Option<String>,
}

async fn invite_one(ortus: &Instance, me: &User, input: &Invite) -> Result<(User, Sent), ApiError> {
    let role = Role::parse(input.role.as_deref().unwrap_or("member"))
        .ok_or_else(|| ApiError::bad_request("The role is member, admin or owner."))?;
    if role > me.role {
        return Err(ApiError::forbidden());
    }
    if input.name.trim().is_empty() {
        return Err(ApiError::bad_request("Give a name."));
    }
    let account = accounts::normal_email(&input.email).unwrap_or_default();
    let send_to = personal(input.send_to.as_deref(), &account)?;
    let store = ortus.store.as_ref();
    let user = accounts::create(
        store,
        &NewUser {
            email: &input.email,
            name: &input.name,
            role,
            state: State::Invited,
            expires_at: None,
        },
        now_ms(),
    )
    .await
    .map_err(|e| match e {
        StoreError::Conflict(_) => {
            ApiError::conflict(format!("{} already has an account.", input.email.trim()))
        }
        StoreError::Database(m) if m.contains("not an email") => {
            ApiError::bad_request(format!("'{}' is not an email address.", input.email))
        }
        _ => ApiError::internal(),
    })?;
    for group in input.groups.iter().flatten() {
        join_group(store, group, &user.id).await?;
    }
    let sent = send_invite(
        ortus,
        &user,
        &me.name,
        input.language.as_deref(),
        send_to,
        true,
    )
    .await?;
    super::audit(
        ortus,
        crate::audit::Event::new("people.invited", crate::audit::Outcome::Done)
            .by(&me.id)
            .on(&user.id)
            .with(
                json!({"role": role.name(), "letter_to": sent.sent_to, "not_sent": sent.not_sent}),
            ),
    )
    .await;
    Ok((user, sent))
}

/// Adds the person to the group named `name`, making the group if needed.
pub async fn join_group(store: &dyn AsyncStore, name: &str, user_id: &str) -> StoreResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(());
    }
    let existing = store
        .query("SELECT id FROM core_groups WHERE name = $1", &[name.into()])
        .await?;
    let id = match existing.first().and_then(|r| r.text(0)) {
        Some(id) => id.to_string(),
        None => {
            let id = format!("g_{}", super::random_hex(8));
            store.execute("INSERT INTO core_groups (id, name, created_at) VALUES ($1, $2, $3) ON CONFLICT (name) DO NOTHING", &[id.as_str().into(), name.into(), now_ms().into()]).await?;
            store
                .query("SELECT id FROM core_groups WHERE name = $1", &[name.into()])
                .await?
                .first()
                .and_then(|r| r.text(0))
                .unwrap_or_default()
                .to_string()
        }
    };
    store
        .execute("INSERT INTO core_group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT (group_id, user_id) DO NOTHING", &[id.as_str().into(), user_id.into()])
        .await?;
    Ok(())
}

async fn invite(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = me_admin(request, ortus).await?;
    let input: Invite = body(request)?;
    let (user, sent) = invite_one(ortus, &me, &input).await?;
    let mut answer = row(ortus.store.as_ref(), &user).await?;
    if let (Some(answer), Value::Object(extra)) = (answer.as_object_mut(), sent.json()) {
        answer.extend(extra);
    }
    Ok(ok(&answer))
}

/// One person per line: `email,name[,role[,group;group[,personal email]]]`.
/// A header line starting with `email` is skipped. Lines that fail are
/// reported, the rest go in; each invitation's link is in the answer, for
/// a list to hand out.
async fn import(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = me_admin(request, ortus).await?;
    let text = String::from_utf8_lossy(request.body.as_bytes()).into_owned();
    let (mut added, mut invited, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || (number == 0 && line.to_lowercase().starts_with("email")) {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let input = Invite {
            email: fields.first().unwrap_or(&"").to_string(),
            name: fields.get(1).unwrap_or(&"").to_string(),
            role: fields
                .get(2)
                .filter(|r| !r.is_empty())
                .map(|r| r.to_string()),
            groups: fields
                .get(3)
                .map(|g| g.split(';').map(str::to_string).collect()),
            language: None,
            send_to: fields
                .get(4)
                .filter(|a| !a.is_empty())
                .map(|a| a.to_string()),
        };
        match invite_one(ortus, &me, &input).await {
            Ok((user, sent)) => {
                invited.push(json!({
                    "email": user.email, "name": user.name, "link": sent.link,
                    "expires_at": sent.expires_at, "sent_to": sent.sent_to, "not_sent": sent.not_sent,
                }));
                added.push(user.email);
            }
            Err(e) => failed.push(json!({"line": number + 1, "reason": e.message})),
        }
    }
    Ok(ok(
        &json!({ "added": added, "invited": invited, "failed": failed }),
    ))
}

#[derive(Deserialize)]
struct Change {
    name: Option<String>,
    role: Option<String>,
    state: Option<String>,
    quota_bytes: Option<i64>,
    #[serde(default)]
    clear_quota: bool,
}

async fn change(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = me_admin(request, ortus).await?;
    let id = params.get("id").unwrap_or_default().to_string();
    let store = ortus.store.as_ref();
    let user = accounts::by_id(store, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("There is no such person."))?;
    // Nobody acts on someone above them; owners stay with owners.
    if user.role > me.role {
        return Err(ApiError::forbidden());
    }
    let input: Change = body(request)?;
    // Everything is checked before anything is written.
    let role = match input.role.as_deref() {
        None => None,
        Some(role) => {
            let role = Role::parse(role)
                .ok_or_else(|| ApiError::bad_request("The role is member, admin or owner."))?;
            if role > me.role {
                return Err(ApiError::forbidden());
            }
            if user.role == Role::Owner && role != Role::Owner && owners(store).await? <= 1 {
                return Err(ApiError {
                    status: 409,
                    code: "last_owner",
                    message: "An installation keeps at least one owner.".into(),
                });
            }
            Some(role)
        }
    };
    let state = match input.state.as_deref() {
        None => None,
        Some("suspended") => {
            if id == me.id {
                return Err(ApiError::bad_request("You cannot suspend yourself."));
            }
            Some(State::Suspended)
        }
        // Back from a suspension: someone who never set up a way to sign
        // in is invited again (their invitation must still be accepted).
        Some("active") if user.state == State::Suspended => {
            Some(if has_sign_in(store, &id).await? {
                State::Active
            } else {
                State::Invited
            })
        }
        Some("active") if user.state == State::Active => Some(State::Active),
        Some(_) => {
            return Err(ApiError::bad_request(
                "The state is suspended or active (from suspended).",
            ))
        }
    };
    if let Some(name) = input.name.as_deref().filter(|n| !n.trim().is_empty()) {
        accounts::set(store, &id, Field::Name(name)).await?;
    }
    if let Some(role) = role {
        accounts::set(store, &id, Field::Role(role)).await?;
    }
    if let Some(state) = state {
        accounts::set(store, &id, Field::State(state)).await?;
        if state == State::Suspended {
            sessions::end_all(store, &id).await?;
            store
                .execute(
                    "DELETE FROM core_oidc_tokens WHERE user_id = $1",
                    &[id.as_str().into()],
                )
                .await?;
        }
    }
    if input.clear_quota || input.quota_bytes.is_some() {
        let quota = if input.clear_quota {
            None
        } else {
            input.quota_bytes.filter(|q| *q > 0)
        };
        store
            .execute(
                "UPDATE core_users SET quota_bytes = $1 WHERE id = $2",
                &[quota.into(), id.as_str().into()],
            )
            .await?;
    }
    super::audit(
        ortus,
        crate::audit::Event::new("people.changed", crate::audit::Outcome::Done)
            .by(&me.id)
            .on(&id)
            .with(
                json!({"role": input.role, "state": input.state, "quota_bytes": input.quota_bytes}),
            ),
    )
    .await;
    let user = accounts::by_id(store, &id)
        .await?
        .ok_or_else(ApiError::internal)?;
    Ok(ok(&row(store, &user).await?))
}

/// Whether the person has any way to sign in of their own.
async fn has_sign_in(store: &dyn AsyncStore, id: &str) -> StoreResult<bool> {
    let rows = store
        .query(
            "SELECT (SELECT COUNT(*) FROM core_credentials WHERE user_id = $1) + (SELECT COUNT(*) FROM core_external_links WHERE user_id = $1)",
            &[id.into()],
        )
        .await?;
    Ok(rows.first().and_then(|r| r.int(0)).unwrap_or(0) > 0)
}

async fn owners(store: &dyn AsyncStore) -> StoreResult<i64> {
    let rows = store
        .query(
            "SELECT COUNT(*) FROM core_users WHERE role = 'owner' AND state = 'active'",
            &[],
        )
        .await?;
    Ok(rows.first().and_then(|r| r.int(0)).unwrap_or(0))
}

/// Signs the person out everywhere and sends a fresh invitation, so they
/// set up their sign-in again (a lost phone, a forgotten password).
async fn reset(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = me_admin(request, ortus).await?;
    let id = params.get("id").unwrap_or_default().to_string();
    let store = ortus.store.as_ref();
    let user = accounts::by_id(store, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("There is no such person."))?;
    if user.role > me.role {
        return Err(ApiError::forbidden());
    }
    if user.role == Role::Owner && user.state == State::Active && owners(store).await? <= 1 {
        return Err(ApiError {
            status: 409,
            code: "last_owner",
            message: "An installation keeps at least one owner who can sign in.".into(),
        });
    }
    sessions::end_all(store, &id).await?;
    // Every way they signed in goes, outside providers too.
    for sql in [
        "DELETE FROM core_credentials WHERE user_id = $1",
        "DELETE FROM core_external_links WHERE user_id = $1",
        "DELETE FROM core_oidc_tokens WHERE user_id = $1",
    ] {
        store.execute(sql, &[id.as_str().into()]).await?;
    }
    accounts::set(store, &id, Field::State(State::Invited)).await?;
    let options = letter_options(request, &user)?;
    let sent = send_invite(
        ortus,
        &user,
        &me.name,
        user.language.as_deref(),
        options.send_to,
        options.send,
    )
    .await?;
    super::audit(
        ortus,
        crate::audit::Event::new("people.sign_in_reset", crate::audit::Outcome::Done)
            .by(&me.id)
            .on(&id),
    )
    .await;
    let mut answer = sent.json();
    answer["reset"] = json!(id);
    Ok(ok(&answer))
}

#[derive(Deserialize, Default)]
struct LetterInput {
    send: Option<bool>,
    send_to: Option<String>,
}

/// What a new link's letter should do: send (unless told not to), to a
/// personal address when one is given.
struct LetterOptions {
    send: bool,
    send_to: Option<String>,
}

fn letter_options(request: &Request, user: &User) -> Result<LetterOptions, ApiError> {
    let input: LetterInput = if request.body.as_bytes().iter().all(u8::is_ascii_whitespace) {
        LetterInput::default()
    } else {
        body(request)?
    };
    Ok(LetterOptions {
        send: input.send.unwrap_or(true),
        send_to: personal(input.send_to.as_deref(), &user.email)?,
    })
}

/// Someone invited who has not joined: their invitation, by id, if the
/// administrator may act on them.
async fn invited(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<(User, User), ApiError> {
    let me = me_admin(request, ortus).await?;
    let id = params.get("id").unwrap_or_default().to_string();
    let user = accounts::by_id(ortus.store.as_ref(), &id)
        .await?
        .ok_or_else(|| ApiError::not_found("There is no such person."))?;
    if user.role > me.role {
        return Err(ApiError::forbidden());
    }
    if user.state != State::Invited {
        return Err(ApiError {
            status: 409,
            code: "not_invited",
            message: "This person has joined; reset their sign-in instead.".into(),
        });
    }
    Ok((me, user))
}

/// A new link for someone invited (the old one stops working): sent again
/// ({send_to} to a personal address, else to where the last letter went),
/// or only shown ({send: false}).
async fn new_link(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let (me, user) = invited(request, params, ortus).await?;
    let store = ortus.store.as_ref();
    let mut options = letter_options(request, &user)?;
    if options.send_to.is_none() {
        options.send_to = store
            .query(
                "SELECT send_to FROM core_invites WHERE user_id = $1",
                &[user.id.as_str().into()],
            )
            .await?
            .first()
            .and_then(|r| r.text(0))
            .map(str::to_string);
    }
    let sent = send_invite(
        ortus,
        &user,
        &me.name,
        user.language.as_deref(),
        options.send_to,
        options.send,
    )
    .await?;
    super::audit(
        ortus,
        crate::audit::Event::new("people.invitation_renewed", crate::audit::Outcome::Done)
            .by(&me.id)
            .on(&user.id)
            .with(json!({"letter_to": sent.sent_to, "not_sent": sent.not_sent})),
    )
    .await;
    Ok(ok(&sent.json()))
}

/// Withdraws someone's invitation: its link stops working. They stay
/// listed as invited, so a new link can be made later.
async fn withdraw(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let (me, user) = invited(request, params, ortus).await?;
    ortus
        .store
        .execute(
            "DELETE FROM core_invites WHERE user_id = $1",
            &[user.id.as_str().into()],
        )
        .await?;
    super::audit(
        ortus,
        crate::audit::Event::new("people.invitation_withdrawn", crate::audit::Outcome::Done)
            .by(&me.id)
            .on(&user.id),
    )
    .await;
    Ok(ok(&json!({ "withdrawn": user.id })))
}

async fn devices(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = me_admin(request, ortus).await?;
    let id = params.get("id").unwrap_or_default().to_string();
    let user = accounts::by_id(ortus.store.as_ref(), &id)
        .await?
        .ok_or_else(|| ApiError::not_found("There is no such person."))?;
    if user.role > me.role {
        return Err(ApiError::forbidden());
    }
    let list = sessions::list(ortus.store.as_ref(), &id).await?;
    let devices: Vec<Value> = list.iter().map(|s| json!({"agent": s.agent, "address": s.address, "methods": s.methods, "last_seen_at": s.last_seen_at, "created_at": s.created_at})).collect();
    Ok(ok(&json!({ "devices": devices })))
}

#[derive(Deserialize)]
struct Accept {
    token: String,
}

/// The invited person follows their link: the account becomes active and
/// signed in, and the page goes on to add a passkey.
async fn accept(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let input: Accept = body(request)?;
    let store = ortus.store.as_ref();
    let hash = sha256_hex(&input.token);
    let mut tx = store.begin().await?;
    let rows = tx
        .query(
            "SELECT user_id, expires_at FROM core_invites WHERE token_hash = $1",
            &[hash.as_str().into()],
        )
        .await?;
    tx.execute(
        "DELETE FROM core_invites WHERE token_hash = $1",
        &[hash.as_str().into()],
    )
    .await?;
    tx.commit().await?;
    let expired = || ApiError {
        status: 410,
        code: "invitation_expired",
        message: "This invitation has expired or was used. Ask for a new one.".into(),
    };
    let row = rows.first().ok_or_else(expired)?;
    if row.int(1).unwrap_or_default() < now_ms() {
        return Err(expired());
    }
    let user_id = row.text(0).unwrap_or_default().to_string();
    let user = accounts::by_id(store, &user_id)
        .await?
        .filter(|u| u.state == State::Invited)
        .ok_or_else(expired)?;
    accounts::set(store, &user.id, Field::State(State::Active)).await?;
    super::audit(
        ortus,
        crate::audit::Event::new("people.invitation_accepted", crate::audit::Outcome::Done)
            .by(&user.id)
            .on(&user.id),
    )
    .await;
    let user = accounts::by_id(store, &user_id)
        .await?
        .ok_or_else(ApiError::internal)?;
    Ok(signed_in_with(
        request,
        connection,
        ortus,
        &user,
        "invitation",
        json!({ "next": "add_sign_in_method" }),
    )
    .await)
}

async fn groups(_: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let rows = ortus
        .store
        .query("SELECT g.id, g.name, (SELECT COUNT(*) FROM core_group_members m WHERE m.group_id = g.id) FROM core_groups g ORDER BY g.name", &[])
        .await?;
    let list: Vec<Value> = rows
        .iter()
        .map(|r| json!({"id": r.text(0), "name": r.text(1), "members": r.int(2)}))
        .collect();
    Ok(ok(&json!({ "groups": list })))
}

macro_rules! h {
    ($name:ident, $inner:ident) => {
        fn $name(
            request: Request,
            _: PathParams,
            _: ConnectionInfo,
            ortus: Arc<Instance>,
        ) -> BoxFuture<'static, Response> {
            Box::pin(async move { answer(&request, $inner(&request, &ortus).await) })
        }
    };
    ($name:ident, $inner:ident, params) => {
        fn $name(
            request: Request,
            params: PathParams,
            _: ConnectionInfo,
            ortus: Arc<Instance>,
        ) -> BoxFuture<'static, Response> {
            Box::pin(async move { answer(&request, $inner(&request, &params, &ortus).await) })
        }
    };
    ($name:ident, $inner:ident, connection) => {
        fn $name(
            request: Request,
            _: PathParams,
            connection: ConnectionInfo,
            ortus: Arc<Instance>,
        ) -> BoxFuture<'static, Response> {
            Box::pin(async move { answer(&request, $inner(&request, &connection, &ortus).await) })
        }
    };
}

h!(h_list, list);
h!(h_invite, invite);
h!(h_import, import);
h!(h_change, change, params);
h!(h_reset, reset, params);
h!(h_new_link, new_link, params);
h!(h_withdraw, withdraw, params);
h!(h_devices, devices, params);
h!(h_accept, accept, connection);
h!(h_groups, groups);

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    let r = |method, path, id, summary, access, handler| Route {
        method,
        path,
        id,
        tag: "people",
        summary,
        access,
        response: any_object,
        handler,
    };
    vec![
        r(Method::Get, "/admin/users", "listPeople", "Everyone, with role, state, how they sign in, last activity, quota and groups. Query: q (name, address or group), filter (no_2fa, near_quota).", Access::Admin, h_list as crate::api::Handler),
        r(Method::Post, "/admin/users", "invitePerson", "Invite someone: {email, name, role, groups, language, send_to}. The answer has the link (and as a QR code), when it expires, and where the letter went (send_to, a personal address; none when the address is a mailbox here).", Access::Admin, h_invite),
        r(Method::Post, "/admin/users/import", "importPeople", "Invite many: CSV text, one per line: email,name[,role[,group;group[,personal email]]]. The answer lists each invitation's link.", Access::Admin, h_import),
        r(Method::Patch, "/admin/users/:id", "changePerson", "Change a person: {name, role, state (suspended or active), quota_bytes, clear_quota}.", Access::Admin, h_change),
        r(Method::Post, "/admin/users/:id/reset", "resetSignIn", "Sign a person out everywhere, remove their sign-in methods and make a new invitation: {send, send_to}, both optional.", Access::Admin, h_reset),
        r(Method::Post, "/admin/users/:id/invitation", "renewInvitation", "A new link for someone invited (the old one stops working): {send (default true), send_to}.", Access::Admin, h_new_link),
        r(Method::Delete, "/admin/users/:id/invitation", "withdrawInvitation", "Withdraw someone's invitation: its link stops working.", Access::Admin, h_withdraw),
        r(Method::Get, "/admin/users/:id/devices", "listPersonDevices", "Where a person is signed in.", Access::Admin, h_devices),
        r(Method::Get, "/admin/groups", "listGroups", "The groups and how many people are in each.", Access::Admin, h_groups),
        r(Method::Post, "/invitation/accept", "acceptInvitation", "Follow an invitation link: {token}. Signs in; next, add a sign-in method.", Access::Public, h_accept),
    ]
}
