//! One running installation: its settings, database, files, apps and
//! services, shared by every request and job.

use std::sync::Arc;

use crate::app::{self, App, Switches};
use crate::config::Config;
use crate::files::Files;
use crate::services::Services;
use crate::storage::{install, AsyncStore, Schema};

pub struct Instance {
    pub config: Config,
    pub store: Arc<dyn AsyncStore>,
    pub files: Arc<Files>,
    pub apps: Vec<Arc<dyn App>>,
    pub switches: Switches,
    pub services: Services,
}

/// Every schema of the core and of `apps`, in the order they install.
pub fn schemas(apps: &[Arc<dyn App>]) -> Vec<Schema> {
    let mut all = crate::schemas();
    all.extend(apps.iter().map(|a| app::schema_of(a.as_ref())));
    all
}

impl Instance {
    /// Opens the database and files of `config`, brings every schema up to
    /// date, and lets each app provide its services.
    pub async fn open(
        config: Config,
        apps: Vec<Arc<dyn App>>,
        now_ms: i64,
    ) -> Result<Arc<Self>, String> {
        app::check(&apps)?;
        let store = config.open_store().await.map_err(|e| e.to_string())?;
        let files = Arc::new(config.open_files().map_err(|e| e.to_string())?);
        install(store.as_ref(), &schemas(&apps), now_ms)
            .await
            .map_err(|e| e.to_string())?;
        Ok(Self::assemble(config, store, files, apps))
    }

    /// An instance on an open database whose schemas are installed.
    pub fn assemble(
        config: Config,
        store: Arc<dyn AsyncStore>,
        files: Arc<Files>,
        apps: Vec<Arc<dyn App>>,
    ) -> Arc<Self> {
        let switches = Switches::new(Arc::clone(&store), config.apps_off.clone());
        let instance = Arc::new(Self {
            config,
            store,
            files,
            apps,
            switches,
            services: Services::new(),
        });
        for app in &instance.apps {
            app.provide(&instance, &instance.services);
        }
        instance
    }

    /// The app with this id, if installed.
    pub fn app(&self, id: &str) -> Option<&Arc<dyn App>> {
        self.apps.iter().find(|app| app.info().id == id)
    }

    /// The apps that are installed and on, in switcher order.
    pub async fn apps_on(&self) -> Vec<app::AppInfo> {
        let mut on = Vec::new();
        for app in &self.apps {
            let info = app.info();
            if self.switches.is_on(info.id).await.unwrap_or(false) {
                on.push(info);
            }
        }
        on.sort_by_key(|info| (info.group, info.order, info.id));
        on
    }

    /// The provider of a service, or the "not installed" answer.
    pub async fn service<T: ?Sized + Send + Sync + 'static>(
        &self,
    ) -> Result<Arc<T>, crate::services::NotInstalled> {
        self.services.get::<T>(&self.switches).await
    }
}

/// Instances for tests: in-memory SQLite and a fresh files directory.
pub mod testing {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use super::{schemas, Instance};
    use crate::app::App;
    use crate::config::Config;
    use crate::files::Files;
    use crate::storage::{install, SqliteStore};

    /// A fresh installation with `apps`, every schema installed. Its files
    /// directory is under the system's temporary directory.
    pub fn instance(apps: Vec<Arc<dyn App>>) -> Arc<Instance> {
        instance_with(
            Config::load(None, &|_| None).expect("the default configuration"),
            apps,
        )
    }

    /// As [`instance`], with these settings (the database and files are
    /// always fresh ones).
    pub fn instance_with(config: Config, apps: Vec<Arc<dyn App>>) -> Arc<Instance> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ortus-core-instance-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let store: Arc<dyn crate::storage::AsyncStore> =
            Arc::new(SqliteStore::in_memory().expect("an in-memory database"));
        crate::app::check(&apps).expect("apps that fit together");
        crate::runtime::block_on(install(store.as_ref(), &schemas(&apps), 0)).expect("the schemas");
        let files = Arc::new(Files::local(&dir).expect("a files directory"));
        Instance::assemble(config, store, files, apps)
    }
}

/// A signed-in person for tests.
pub mod testing_auth {
    use std::sync::Arc;

    use super::Instance;
    use crate::auth::accounts::{self, NewUser, Role, State};
    use crate::auth::sessions::{self, Lifetime};
    use crate::runtime::block_on;

    /// Creates an active account with `role` and returns a session token
    /// for it.
    pub fn signed_in(instance: &Arc<Instance>, email: &str, role: Role) -> String {
        let store = instance.store.as_ref();
        let user = block_on(accounts::create(
            store,
            &NewUser {
                email,
                name: email,
                role,
                state: State::Active,
                expires_at: None,
            },
            0,
        ))
        .expect("a new account");
        block_on(sessions::start(
            store,
            &user.id,
            "test",
            "",
            "",
            Lifetime::default(),
            crate::program::now_ms(),
        ))
        .expect("a session")
    }
}

/// Instances on a store a test chose (to run API tests on PostgreSQL).
pub mod testing_store {
    use std::sync::Arc;

    use super::{schemas, Instance};
    use crate::app::App;
    use crate::config::Config;
    use crate::files::Files;
    use crate::storage::{install, AsyncStore};

    pub fn instance(store: Arc<dyn AsyncStore>, apps: Vec<Arc<dyn App>>) -> Arc<Instance> {
        let config = Config::load(None, &|_| None).expect("the default configuration");
        crate::runtime::block_on(install(store.as_ref(), &schemas(&apps), 0)).expect("the schemas");
        let dir = std::env::temp_dir().join(format!(
            "ortus-core-store-instance-{}-{}",
            std::process::id(),
            crate::auth::random_hex(4)
        ));
        let files = Arc::new(Files::local(&dir).expect("a files directory"));
        Instance::assemble(config, store, files, apps)
    }
}
