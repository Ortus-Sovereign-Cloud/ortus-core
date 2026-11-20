//! The app interface: how mail, drive, chat and the other apps plug into
//! the core.
//!
//! An app is a value that implements [`App`]. It tells the core its place
//! in the app switcher, its database tables, its API routes, its kinds of
//! background job, the listeners it opens for a role (mail opens SMTP and
//! IMAP for the mail role) and the services it offers other apps. Apps
//! depend on the core only, never on each other: an app that needs another
//! one asks the core for a service (see [`crate::services`]), and gets a
//! "not installed" answer when that app is absent or switched off.
//!
//! Every app has an on and off switch ([`Switches`]). A switched-off app
//! is not in the switcher, its API answers `not_installed`, its jobs wait
//! in the queue, and its listeners are not opened at the next start. Its
//! data stays where it is.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::api::Route;
use crate::config::Role;
use crate::instance::Instance;
use crate::jobs;
use crate::services::Services;
use crate::storage::{AsyncStore, AsyncTx, BoxFuture, Migration, Schema, StoreResult};

/// The groups of the app switcher, in the order it shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Communicate,
    Organise,
    Create,
    Organisation,
}

impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Group::Communicate => "communicate",
            Group::Organise => "organise",
            Group::Create => "create",
            Group::Organisation => "organisation",
        }
    }
}

/// An app's entry in the app switcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppInfo {
    /// Stable id, lowercase letters only: `mail`, `calendar`, `drive`.
    /// It is the first part of the app's API paths, job kinds and tables.
    pub id: &'static str,
    pub group: Group,
    /// Where the app lives in the web client, as `/mail`.
    pub path: &'static str,
    /// Position within its group, lowest first.
    pub order: u32,
}

/// A section an app adds to the admin console, such as mail's Domains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdminSection {
    /// Under `/admin/`, as `/admin/domains`.
    pub path: &'static str,
    /// The element the app's client defines for it, `ortus-<id>-...`.
    pub element: &'static str,
    /// The key of its name in the app's texts.
    pub label: &'static str,
}

/// Work an app does for one role, such as accepting SMTP connections for
/// the mail role. It runs until `stop` resolves.
pub struct RoleTask {
    pub role: Role,
    /// Shown in the log: `smtp`, `imap`.
    pub name: &'static str,
    pub run: fn(Arc<Instance>, Stop) -> BoxFuture<'static, Result<(), String>>,
}

/// Tells a role task to stop.
pub type Stop = tokio::sync::watch::Receiver<bool>;

/// A kind of background job and what runs it. The kind must start with
/// the app's id and a dot: `mail.send`.
pub struct JobKind {
    pub kind: String,
    pub handler: jobs::Handler,
}

/// A kind of background job whose handler runs outside any transaction
/// (see [`jobs::OutsideHandler`]): work that waits on the network.
pub struct OutsideJobKind {
    pub kind: String,
    pub handler: jobs::OutsideHandler,
}

/// An Ortus app. Only [`info`](App::info) is required.
pub trait App: Send + Sync + 'static {
    fn info(&self) -> AppInfo;

    /// The app's tables, as migrations. Table names start with the id.
    fn migrations(&self) -> &'static [Migration] {
        &[]
    }

    /// The app's API routes. Their paths start with `/<id>`, as
    /// `/mail/messages`; the core puts them under `/api/v1`.
    fn routes(&self) -> Vec<Route> {
        Vec::new()
    }

    /// The kinds of background job the app runs.
    fn jobs(&self, _instance: &Arc<Instance>) -> Vec<JobKind> {
        Vec::new()
    }

    /// Kinds of background job that run outside any transaction, because
    /// they wait on the network. The kind must start with the app's id.
    fn outside_jobs(&self, _instance: &Arc<Instance>) -> Vec<OutsideJobKind> {
        Vec::new()
    }

    /// Sections the app adds to the admin console.
    fn admin_sections(&self) -> Vec<AdminSection> {
        Vec::new()
    }

    /// Jobs to run on a schedule: (kind, every so many milliseconds, at
    /// least a minute). The kind must be one of [`App::jobs`].
    fn periodic(&self) -> Vec<(&'static str, i64)> {
        Vec::new()
    }

    /// The listeners and loops the app runs for each role.
    fn role_tasks(&self) -> Vec<RoleTask> {
        Vec::new()
    }

    /// Offers services to other apps, once at start.
    fn provide(&self, _instance: &Arc<Instance>, _services: &Services) {}

    /// A file of the app's built web client, by its path under the app's
    /// assets (`app.js` defines the `ortus-<id>-app` element the shell
    /// shows at the app's path). Usually from a `rust_embed` folder.
    fn web_file(&self, _path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
        None
    }

    /// The daily retention sweep: removes what is past its period and on
    /// no hold (`rules.may_purge`), through `tx` only (files go by queuing
    /// jobs in `tx`), and answers how many things went.
    fn purge<'a>(
        &'a self,
        _tx: &'a mut dyn AsyncTx,
        _rules: &'a crate::retention::Rules,
    ) -> BoxFuture<'a, Result<u64, String>> {
        Box::pin(async { Ok(0) })
    }

    /// Deletes everything the app keeps for an account that is being
    /// deleted (an expired demo account, or an administrator's deletion).
    /// It works through `tx`, the deletion's own transaction, and must not
    /// use the store otherwise; files go by queuing jobs in `tx`. What sits
    /// in a channel on legal hold (`rules.held_channels`) stays.
    fn forget_user<'a>(
        &'a self,
        _tx: &'a mut dyn AsyncTx,
        _user_id: &'a str,
        _rules: &'a crate::retention::Rules,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

/// The schema of an app, named `<id>` in the migration history.
pub fn schema_of(app: &dyn App) -> Schema {
    Schema {
        app: app.info().id,
        migrations: app.migrations(),
    }
}

/// Why a set of apps cannot run together.
pub fn check(apps: &[Arc<dyn App>]) -> Result<(), String> {
    let mut seen = Vec::new();
    for app in apps {
        let info = app.info();
        if info.id.is_empty() || !info.id.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(format!(
                "app id '{}' must be lowercase letters only",
                info.id
            ));
        }
        if matches!(info.id, "core" | "api" | "assets" | "admin" | "apps") {
            return Err(format!("app id '{}' is reserved", info.id));
        }
        if seen.contains(&info.id) {
            return Err(format!("two apps have the id '{}'", info.id));
        }
        seen.push(info.id);
        if info.path != format!("/{}", info.id) {
            return Err(format!("app '{}' must live at /{}", info.id, info.id));
        }
        let core_sections = [
            "/admin",
            "/admin/users",
            "/admin/sign-in",
            "/admin/sovereignty",
        ];
        for section in app.admin_sections() {
            let name = section.path.strip_prefix("/admin/").unwrap_or_default();
            if name.is_empty()
                || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
                || core_sections.contains(&section.path)
            {
                return Err(format!(
                    "app '{}' has the admin section {}, not a free /admin/<name>",
                    info.id, section.path
                ));
            }
            if !section.element.starts_with(&format!("ortus-{}-", info.id)) {
                return Err(format!(
                    "app '{}' names the element {}, not ortus-{}-...",
                    info.id, section.element, info.id
                ));
            }
        }
        for route in app.routes() {
            let own = route.path == format!("/{}", info.id)
                || route.path.starts_with(&format!("/{}/", info.id));
            if !own {
                return Err(format!(
                    "app '{}' has the route {}, outside /{}",
                    info.id, route.path, info.id
                ));
            }
        }
    }
    Ok(())
}

const SWITCHES: &[Migration] = &[Migration {
    version: 1,
    name: "app switches",
    sql: "CREATE TABLE core_app_switches (
        app TEXT PRIMARY KEY,
        enabled BIGINT NOT NULL,
        changed_at BIGINT NOT NULL
    )",
}];

/// The schema of the switch table.
pub const SWITCHES_SCHEMA: Schema = Schema {
    app: "core.apps",
    migrations: SWITCHES,
};

/// How long a program trusts what it last read about the switches. Other
/// programs on the same database see a change within this time.
const FRESH: Duration = Duration::from_secs(2);

/// The on and off switch of every app.
///
/// A switch an administrator has set is kept in the database and wins.
/// Otherwise an app is on, unless the configuration names it in
/// `apps.off`.
pub struct Switches {
    store: Arc<dyn AsyncStore>,
    off_by_default: Vec<String>,
    cache: RwLock<Option<(Instant, HashMap<String, bool>)>>,
}

impl Switches {
    pub fn new(store: Arc<dyn AsyncStore>, off_by_default: Vec<String>) -> Self {
        Self {
            store,
            off_by_default,
            cache: RwLock::new(None),
        }
    }

    async fn current(&self) -> StoreResult<HashMap<String, bool>> {
        if let Some((read, map)) = self
            .cache
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            if read.elapsed() < FRESH {
                return Ok(map.clone());
            }
        }
        let rows = self
            .store
            .query("SELECT app, enabled FROM core_app_switches", &[])
            .await?;
        let map: HashMap<String, bool> = rows
            .iter()
            .filter_map(|row| Some((row.text(0)?.to_string(), row.int(1)? != 0)))
            .collect();
        *self.cache.write().unwrap_or_else(|p| p.into_inner()) =
            Some((Instant::now(), map.clone()));
        Ok(map)
    }

    /// Whether the app `id` is on.
    pub async fn is_on(&self, id: &str) -> StoreResult<bool> {
        Ok(match self.current().await?.get(id) {
            Some(on) => *on,
            None => !self.off_by_default.iter().any(|off| off == id),
        })
    }

    /// Turns the app `id` on or off.
    pub async fn set(&self, id: &str, on: bool, now_ms: i64) -> StoreResult<()> {
        let mut tx = self.store.begin().await?;
        tx.execute("DELETE FROM core_app_switches WHERE app = $1", &[id.into()])
            .await?;
        tx.execute(
            "INSERT INTO core_app_switches (app, enabled, changed_at) VALUES ($1, $2, $3)",
            &[id.into(), i64::from(on).into(), now_ms.into()],
        )
        .await?;
        tx.commit().await?;
        *self.cache.write().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(())
    }
}
