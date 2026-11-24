//! The people and groups of the installation, for apps: who someone is,
//! finding colleagues, groups and their members, storage quotas. Apps
//! read these through these functions, never the core's tables, so the
//! tables can change without breaking an app pinned to an older core.

use std::collections::BTreeMap;

use crate::storage::{AsyncStore, AsyncTx, Row, StoreResult};

/// A colleague as apps show them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub email: String,
}

/// A group, with how many active or invited people are in it (0 where
/// not counted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub members: i64,
}

fn person(r: &Row) -> Person {
    Person {
        id: r.text(0).unwrap_or_default().into(),
        name: r.text(1).unwrap_or_default().into(),
        email: r.text(2).unwrap_or_default().into(),
    }
}

/// `%text%` for LIKE, lowercase, with the wildcards in it taken literally.
fn like(text: &str) -> String {
    let escaped: String = text
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '%' | '_' | '\\'))
        .collect();
    format!("%{escaped}%")
}

/// Which people count: the active ones, and those invited but not yet in
/// when `invited`.
fn states(invited: bool) -> &'static str {
    if invited {
        "('active', 'invited')"
    } else {
        "('active')"
    }
}

/// Colleagues whose name or address contains `text` (all when empty), by
/// name; only members of `group` when given.
pub async fn find(
    store: &dyn AsyncStore,
    text: &str,
    group: Option<&str>,
    invited: bool,
    limit: i64,
) -> StoreResult<Vec<Person>> {
    let states = states(invited);
    let rows = match group {
        Some(g) => {
            store
                .query(
                    &format!("SELECT u.id, u.name, u.email FROM core_users u JOIN core_group_members m ON m.user_id = u.id WHERE m.group_id = $2 AND u.state IN {states} AND (LOWER(u.name) LIKE $1 OR LOWER(u.email) LIKE $1) ORDER BY u.name, u.email LIMIT $3"),
                    &[like(text).into(), g.into(), limit.into()],
                )
                .await?
        }
        None => {
            store
                .query(
                    &format!("SELECT id, name, email FROM core_users WHERE state IN {states} AND (LOWER(name) LIKE $1 OR LOWER(email) LIKE $1) ORDER BY name, email LIMIT $2"),
                    &[like(text).into(), limit.into()],
                )
                .await?
        }
    };
    Ok(rows.iter().map(person).collect())
}

/// How many people there are (active, and invited when `invited`).
pub async fn count(store: &dyn AsyncStore, invited: bool) -> StoreResult<i64> {
    Ok(store
        .query(
            &format!(
                "SELECT COUNT(*) FROM core_users WHERE state IN {}",
                states(invited)
            ),
            &[],
        )
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0))
}

/// The account an address belongs to, if one does (an account still
/// confirming its address does not count).
pub async fn id_by_email(store: &dyn AsyncStore, email: &str) -> StoreResult<Option<String>> {
    let Some(email) = crate::auth::accounts::normal_email(email) else {
        return Ok(None);
    };
    Ok(store
        .query(
            "SELECT id FROM core_users WHERE email = $1 AND state <> 'unverified'",
            &[email.into()],
        )
        .await?
        .first()
        .and_then(|r| r.text(0).map(str::to_string)))
}

/// The groups someone is in, by name.
pub async fn groups_of(store: &dyn AsyncStore, user: &str) -> StoreResult<Vec<Group>> {
    Ok(store
        .query(
            "SELECT g.id, g.name FROM core_groups g JOIN core_group_members m ON m.group_id = g.id WHERE m.user_id = $1 ORDER BY g.name",
            &[user.into()],
        )
        .await?
        .iter()
        .map(|r| Group {
            id: r.text(0).unwrap_or_default().into(),
            name: r.text(1).unwrap_or_default().into(),
            members: 0,
        })
        .collect())
}

/// One group, if it exists.
pub async fn group(store: &dyn AsyncStore, id: &str) -> StoreResult<Option<Group>> {
    Ok(store
        .query(
            "SELECT id, name FROM core_groups WHERE id = $1",
            &[id.into()],
        )
        .await?
        .first()
        .map(|r| Group {
            id: r.text(0).unwrap_or_default().into(),
            name: r.text(1).unwrap_or_default().into(),
            members: 0,
        }))
}

/// Every group with how many active or invited people are in it, by name.
pub async fn groups(store: &dyn AsyncStore) -> StoreResult<Vec<Group>> {
    Ok(store
        .query(
            "SELECT g.id, g.name, COUNT(u.id) FROM core_groups g LEFT JOIN core_group_members m ON m.group_id = g.id LEFT JOIN core_users u ON u.id = m.user_id AND u.state IN ('active', 'invited') GROUP BY g.id, g.name ORDER BY g.name",
            &[],
        )
        .await?
        .iter()
        .map(|r| Group {
            id: r.text(0).unwrap_or_default().into(),
            name: r.text(1).unwrap_or_default().into(),
            members: r.int(2).unwrap_or(0),
        })
        .collect())
}

/// The names of each person's groups, by person id.
pub async fn memberships(store: &dyn AsyncStore) -> StoreResult<BTreeMap<String, Vec<String>>> {
    let mut all: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in store
        .query(
            "SELECT m.user_id, g.name FROM core_group_members m JOIN core_groups g ON g.id = m.group_id ORDER BY g.name",
            &[],
        )
        .await?
    {
        if let (Some(user), Some(name)) = (r.text(0), r.text(1)) {
            all.entry(user.into()).or_default().push(name.into());
        }
    }
    Ok(all)
}

/// Someone's storage quota in bytes; `None` when they have none (or there
/// is no such person).
pub async fn quota(store: &dyn AsyncStore, user: &str) -> StoreResult<Option<i64>> {
    Ok(store
        .query(
            "SELECT quota_bytes FROM core_users WHERE id = $1",
            &[user.into()],
        )
        .await?
        .first()
        .and_then(|r| r.int(0)))
}

/// Someone's quota inside a transaction, holding their row until it ends,
/// so two uploads checking the same quota take turns.
pub async fn quota_locked(tx: &mut dyn AsyncTx, user: &str) -> StoreResult<Option<i64>> {
    tx.execute(
        "UPDATE core_users SET quota_bytes = quota_bytes WHERE id = $1",
        &[user.into()],
    )
    .await?;
    Ok(tx
        .query(
            "SELECT quota_bytes FROM core_users WHERE id = $1",
            &[user.into()],
        )
        .await?
        .first()
        .and_then(|r| r.int(0)))
}
