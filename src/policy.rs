//! Classification labels, partner organisations, and the one policy
//! function every app asks before something leaves its reader's hands:
//! sharing, links, forwarding, opening on a phone.
//!
//! ```ignore
//! let decision = policy::decide(&instance, &policy::Ask {
//!     actor: &me.id, action: Action::Share, label: file.label.as_deref(),
//!     audience: &Audience::Outside("resident@mail.example".into()),
//!     channel: Channel::Files, object: &format!("drive:file:{id}"), reason: None,
//! }).await?;
//! if !decision.allowed() { return Err(...) }
//! ```
//!
//! Every refusal, and every warning someone went past with a reason, is in
//! the audit log.

use std::sync::Arc;

use rust_web_server::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::auth::Access;
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture, Migration, Schema, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "labels and partners",
    sql: "CREATE TABLE core_labels (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        rank BIGINT NOT NULL,
        rules TEXT NOT NULL,
        changed_at BIGINT NOT NULL
    );
    CREATE TABLE core_partners (
        domain TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        channels TEXT NOT NULL,
        added_at BIGINT NOT NULL
    );
    INSERT INTO core_labels (id, name, rank, rules, changed_at) VALUES
        ('public', 'PUBLIC', 0, '{\"outside\":\"allow\",\"partners\":\"allow\",\"links\":\"allow\",\"forwarding\":\"allow\",\"phones\":true,\"watermark\":false}', 0),
        ('internal', 'INTERNAL', 1, '{\"outside\":\"warn\",\"partners\":\"allow\",\"links\":\"deny\",\"forwarding\":\"warn\",\"phones\":true,\"watermark\":false}', 0),
        ('official-sensitive', 'OFFICIAL-SENSITIVE', 2, '{\"outside\":\"deny\",\"partners\":\"warn\",\"links\":\"deny\",\"forwarding\":\"deny\",\"phones\":true,\"watermark\":true}', 0),
        ('secret', 'SECRET', 3, '{\"outside\":\"deny\",\"partners\":\"deny\",\"links\":\"deny\",\"forwarding\":\"deny\",\"phones\":false,\"watermark\":true}', 0)",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.policy",
    migrations: MIGRATIONS,
};

/// What a rule lets through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rule {
    Allow,
    /// Allowed with a reason, which is audited.
    Warn,
    Deny,
}

/// A label's rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rules {
    /// To people outside the organisation who are not partners.
    pub outside: Rule,
    /// To partner organisations (the federation allow-list).
    pub partners: Rule,
    /// Links anyone with the address can open.
    pub links: Rule,
    /// Forwarding mail out of the organisation.
    pub forwarding: Rule,
    /// Whether it opens on phones.
    pub phones: bool,
    /// Whether views and downloads carry the reader's name.
    pub watermark: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub rank: i64,
    pub rules: Rules,
}

/// Where something is going.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Audience {
    /// People in this installation.
    Internal,
    /// An outside address or domain (`someone@example.org` or `example.org`).
    Outside(String),
    /// Anyone with the link.
    Anyone,
}

/// What is being done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Share,
    Link,
    Forward,
    OpenOnPhone,
}

/// Which kind of thing travels, for what partners are allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Mail,
    Chat,
    Files,
    Calls,
}

impl Channel {
    pub const ALL: [Channel; 4] = [Channel::Mail, Channel::Chat, Channel::Files, Channel::Calls];

    fn name(self) -> &'static str {
        match self {
            Channel::Mail => "mail",
            Channel::Chat => "chat",
            Channel::Files => "files",
            Channel::Calls => "calls",
        }
    }
}

pub struct Ask<'a> {
    pub actor: &'a str,
    pub action: Action,
    /// The thing's label; none means the installation's default.
    pub label: Option<&'a str>,
    pub audience: &'a Audience,
    pub channel: Channel,
    /// What it is, for the audit log (`drive:file:123`).
    pub object: &'a str,
    /// The reason given to go past a warning.
    pub reason: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Allow {
        watermark: bool,
    },
    /// Allowed once a reason is given (ask again with `reason`).
    NeedsReason {
        label: String,
    },
    Deny {
        label: String,
        rule: &'static str,
    },
}

impl Decision {
    pub fn allowed(&self) -> bool {
        matches!(self, Decision::Allow { .. })
    }
}

/// The label every unlabelled thing has.
pub const DEFAULT_LABEL: &str = "internal";

pub async fn labels(store: &dyn AsyncStore) -> StoreResult<Vec<Label>> {
    Ok(store
        .query(
            "SELECT id, name, rank, rules FROM core_labels ORDER BY rank, id",
            &[],
        )
        .await?
        .iter()
        .filter_map(|r| {
            Some(Label {
                id: r.text(0)?.to_string(),
                name: r.text(1)?.to_string(),
                rank: r.int(2)?,
                rules: serde_json::from_str(r.text(3)?).ok()?,
            })
        })
        .collect())
}

/// The label `id`, or the default one; an unknown label is treated as the
/// strictest there is (never as the most open).
pub async fn label(store: &dyn AsyncStore, id: Option<&str>) -> StoreResult<Label> {
    let all = labels(store).await?;
    let wanted = id.unwrap_or(DEFAULT_LABEL);
    if let Some(found) = all.iter().find(|l| l.id == wanted) {
        return Ok(found.clone());
    }
    Ok(all.into_iter().max_by_key(|l| l.rank).unwrap_or(Label {
        id: wanted.to_string(),
        name: wanted.to_uppercase(),
        rank: i64::MAX,
        rules: Rules {
            outside: Rule::Deny,
            partners: Rule::Deny,
            links: Rule::Deny,
            forwarding: Rule::Deny,
            phones: false,
            watermark: true,
        },
    }))
}

/// The domain of an address or a domain, lowercased.
fn domain_of(target: &str) -> String {
    target
        .rsplit('@')
        .next()
        .unwrap_or(target)
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// Whether `domain` belongs to a partner allowed `channel`.
pub async fn is_partner(
    store: &dyn AsyncStore,
    domain: &str,
    channel: Channel,
) -> StoreResult<bool> {
    let domain = domain_of(domain);
    for r in store
        .query("SELECT domain, channels FROM core_partners", &[])
        .await?
    {
        let partner = r.text(0).unwrap_or_default();
        let matches = domain == partner || domain.ends_with(&format!(".{partner}"));
        if matches
            && r.text(1)
                .unwrap_or_default()
                .split(',')
                .any(|c| c == channel.name())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The decision, without recording it, with the label and the rule that
/// applied.
pub async fn judge(store: &dyn AsyncStore, ask: &Ask<'_>) -> StoreResult<(Decision, Label, Rule)> {
    let label = label(store, ask.label).await?;
    let rules = &label.rules;
    let (rule, which) = match (ask.action, ask.audience) {
        (Action::OpenOnPhone, _) => (
            if rules.phones {
                Rule::Allow
            } else {
                Rule::Deny
            },
            "phones",
        ),
        (Action::Link, _) | (_, Audience::Anyone) => (rules.links, "links"),
        (_, Audience::Internal) => (Rule::Allow, "internal"),
        (Action::Forward, Audience::Outside(_)) => (rules.forwarding, "forwarding"),
        (Action::Share, Audience::Outside(target)) => {
            if is_partner(store, target, ask.channel).await? {
                (rules.partners, "partners")
            } else {
                (rules.outside, "outside")
            }
        }
    };
    let decision = match rule {
        Rule::Allow => Decision::Allow {
            watermark: rules.watermark,
        },
        Rule::Warn if ask.reason.is_some_and(|r| r.trim().len() >= 3) => Decision::Allow {
            watermark: rules.watermark,
        },
        Rule::Warn => Decision::NeedsReason {
            label: label.id.clone(),
        },
        Rule::Deny => Decision::Deny {
            label: label.id.clone(),
            rule: which,
        },
    };
    Ok((decision, label, rule))
}

/// The one policy function: decides, and writes refusals and reasoned
/// overrides to the audit log.
pub async fn decide(instance: &Instance, ask: &Ask<'_>) -> StoreResult<Decision> {
    let (decision, label, rule) = judge(instance.store.as_ref(), ask).await?;
    let audience = match ask.audience {
        Audience::Internal => "internal".to_string(),
        Audience::Anyone => "anyone".to_string(),
        Audience::Outside(target) => domain_of(target),
    };
    let detail = json!({ "action": ask.action, "label": label.id, "audience": audience, "channel": ask.channel, "object": ask.object });
    match &decision {
        Decision::Deny { rule, .. } => {
            let mut detail = detail;
            detail["rule"] = json!(rule);
            crate::auth::audit(
                instance,
                crate::audit::Event::new("policy.refused", crate::audit::Outcome::Refused)
                    .by(ask.actor)
                    .with(detail),
            )
            .await;
        }
        Decision::Allow { .. } if rule == Rule::Warn => {
            let mut detail = detail;
            detail["reason"] = json!(ask.reason.unwrap_or_default());
            crate::auth::audit(
                instance,
                crate::audit::Event::new("policy.overridden", crate::audit::Outcome::Done)
                    .by(ask.actor)
                    .with(detail),
            )
            .await;
        }
        _ => {}
    }
    Ok(decision)
}

// The admin API.

async fn list(_: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let store = ortus.store.as_ref();
    let partners: Vec<Value> = store
        .query("SELECT domain, name, channels, added_at FROM core_partners ORDER BY domain", &[])
        .await?
        .iter()
        .map(|r| json!({ "domain": r.text(0), "name": r.text(1), "channels": r.text(2).unwrap_or_default().split(',').filter(|c| !c.is_empty()).collect::<Vec<_>>(), "added_at": r.int(3) }))
        .collect();
    Ok(ok(
        &json!({ "labels": labels(store).await?, "default": DEFAULT_LABEL, "partners": partners }),
    ))
}

#[derive(Deserialize)]
struct LabelChange {
    name: String,
    rank: i64,
    rules: Rules,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

async fn put_label(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let id = params.get("id").unwrap_or_default().to_string();
    let input: LabelChange = body(request)?;
    let name = input.name.trim();
    if !valid_id(&id)
        || name.is_empty()
        || name.chars().count() > 40
        || !(0..=100).contains(&input.rank)
    {
        return Err(ApiError::bad_request("A label has an id (lowercase letters, digits, dashes), a name of up to 40 characters and a rank of 0 to 100."));
    }
    let store = ortus.store.as_ref();
    let rules = serde_json::to_string(&input.rules).unwrap_or_default();
    let mut tx = store.begin().await?;
    tx.execute(
        "DELETE FROM core_labels WHERE id = $1",
        &[id.as_str().into()],
    )
    .await?;
    tx.execute(
        "INSERT INTO core_labels (id, name, rank, rules, changed_at) VALUES ($1, $2, $3, $4, $5)",
        &[
            id.as_str().into(),
            name.into(),
            input.rank.into(),
            rules.into(),
            now_ms().into(),
        ],
    )
    .await?;
    tx.commit().await?;
    crate::auth::audit(
        ortus,
        crate::audit::Event::new("policy.label_changed", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .with(json!({ "label": id, "name": name, "rank": input.rank, "rules": input.rules })),
    )
    .await;
    list(request, ortus).await
}

async fn delete_label(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let id = params.get("id").unwrap_or_default().to_string();
    if id == DEFAULT_LABEL {
        return Err(ApiError::bad_request("The default label stays."));
    }
    // Things that kept this label are judged by the strictest label from now on.
    let removed = ortus
        .store
        .execute(
            "DELETE FROM core_labels WHERE id = $1",
            &[id.as_str().into()],
        )
        .await?;
    if removed == 0 {
        return Err(ApiError::not_found("There is no such label."));
    }
    crate::auth::audit(
        ortus,
        crate::audit::Event::new("policy.label_removed", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .with(json!({ "label": id })),
    )
    .await;
    list(request, ortus).await
}

#[derive(Deserialize)]
struct Partner {
    domain: String,
    name: String,
    channels: Vec<Channel>,
}

fn valid_domain(domain: &str) -> bool {
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2
        && domain.len() <= 253
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
}

async fn add_partner(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let input: Partner = body(request)?;
    let domain = domain_of(&input.domain);
    let name = input.name.trim();
    if !valid_domain(&domain) || name.is_empty() || input.channels.is_empty() {
        return Err(ApiError::bad_request(
            "A partner has a domain, a name and at least one of mail, chat, files, calls.",
        ));
    }
    let channels: Vec<&str> = Channel::ALL
        .iter()
        .filter(|c| input.channels.contains(c))
        .map(|c| c.name())
        .collect();
    let store = ortus.store.as_ref();
    let mut tx = store.begin().await?;
    tx.execute(
        "DELETE FROM core_partners WHERE domain = $1",
        &[domain.as_str().into()],
    )
    .await?;
    tx.execute(
        "INSERT INTO core_partners (domain, name, channels, added_at) VALUES ($1, $2, $3, $4)",
        &[
            domain.as_str().into(),
            name.into(),
            channels.join(",").into(),
            now_ms().into(),
        ],
    )
    .await?;
    tx.commit().await?;
    crate::auth::audit(
        ortus,
        crate::audit::Event::new("policy.partner_added", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .with(json!({ "domain": domain, "channels": channels })),
    )
    .await;
    list(request, ortus).await
}

async fn remove_partner(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let domain = domain_of(params.get("domain").unwrap_or_default());
    let removed = ortus
        .store
        .execute(
            "DELETE FROM core_partners WHERE domain = $1",
            &[domain.as_str().into()],
        )
        .await?;
    if removed == 0 {
        return Err(ApiError::not_found("There is no such partner."));
    }
    crate::auth::audit(
        ortus,
        crate::audit::Event::new("policy.partner_removed", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .with(json!({ "domain": domain })),
    )
    .await;
    list(request, ortus).await
}

#[derive(Deserialize)]
struct Check {
    action: String,
    label: Option<String>,
    /// An address or domain; `anyone`; or absent for people here.
    to: Option<String>,
    channel: Channel,
    reason: Option<String>,
}

/// Lets a client ask before it offers an action (nothing is recorded:
/// the app's own route decides, and audits, when it acts).
async fn check(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let input: Check = body(request)?;
    let action = match input.action.as_str() {
        "share" => Action::Share,
        "link" => Action::Link,
        "forward" => Action::Forward,
        "open_on_phone" => Action::OpenOnPhone,
        _ => {
            return Err(ApiError::bad_request(
                "The action is share, link, forward or open_on_phone.",
            ))
        }
    };
    let audience = match input.to.as_deref() {
        None | Some("") => Audience::Internal,
        Some("anyone") => Audience::Anyone,
        Some(to) => Audience::Outside(to.to_string()),
    };
    let ask = Ask {
        actor: &me.user.id,
        action,
        label: input.label.as_deref(),
        audience: &audience,
        channel: input.channel,
        object: "check",
        reason: input.reason.as_deref(),
    };
    let (decision, _, _) = judge(ortus.store.as_ref(), &ask).await?;
    Ok(ok(&json!(decision)))
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
}

h!(h_list, list);
h!(h_put_label, put_label, params);
h!(h_delete_label, delete_label, params);
h!(h_add_partner, add_partner);
h!(h_remove_partner, remove_partner, params);
h!(h_check, check);

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    let r = |method, path, id, summary, access, handler| Route {
        method,
        path,
        id,
        tag: "policy",
        summary,
        access,
        response: any_object,
        handler,
    };
    vec![
        r(Method::Get, "/admin/policy", "getPolicy", "The classification labels (by rank, with their rules), the default label and the partner organisations.", Access::Admin, h_list as crate::api::Handler),
        r(Method::Put, "/admin/policy/labels/:id", "putLabel", "Add or change a label: {name, rank, rules: {outside, partners, links, forwarding (allow, warn or deny), phones, watermark}}.", Access::Owner, h_put_label),
        r(Method::Delete, "/admin/policy/labels/:id", "deleteLabel", "Remove a label; what kept it is judged by the strictest label from then on.", Access::Owner, h_delete_label),
        r(Method::Post, "/admin/policy/partners", "addPartner", "Allow a partner organisation: {domain, name, channels: [mail, chat, files, calls]}.", Access::Owner, h_add_partner),
        r(Method::Delete, "/admin/policy/partners/:domain", "removePartner", "Remove a partner organisation.", Access::Owner, h_remove_partner),
        r(Method::Post, "/policy/check", "checkPolicy", "Ask what the policy would answer: {action (share, link, forward, open_on_phone), label, to (an address, a domain, anyone, or absent for people here), channel, reason}. Records nothing.", Access::Signed, h_check),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_are_checked() {
        assert!(valid_domain("health.example.gov"));
        for bad in ["localhost", "-x.org", "a..b", "x_y.org", ""] {
            assert!(!valid_domain(bad), "{bad}");
        }
        assert_eq!("example.org", domain_of("Someone@Example.org."));
    }
}
