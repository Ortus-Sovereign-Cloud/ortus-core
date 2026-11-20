//! The app interface, with a small app that uses every part of it: a
//! switcher entry, a table, API routes, a kind of job, a role task and a
//! service for other apps. And its switch: off, the app is gone from the
//! switcher, its API answers `not_installed`, its jobs wait and its
//! service is "not installed".

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ortus_core::api::{ok, Method, Route};
use ortus_core::app::{self, App, AppInfo, Group, JobKind, RoleTask, Stop};
use ortus_core::config::{Config, Role};
use ortus_core::instance::{testing, Instance};
use ortus_core::interfaces::{ItemRef, Tasks};
use ortus_core::jobs::{self, NewJob};
use ortus_core::program::AppWorker;
use ortus_core::runtime::block_on;
use ortus_core::services::Services;
use ortus_core::storage::{BoxFuture, Migration};
use rust_web_server::prelude::*;
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

static TICKS: AtomicUsize = AtomicUsize::new(0);

struct Notes;

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "notes",
    sql: "CREATE TABLE notes_items (id TEXT PRIMARY KEY, body TEXT NOT NULL)",
}];

fn count(
    _: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let rows = ortus
            .store
            .query("SELECT COUNT(*) FROM notes_items", &[])
            .await
            .unwrap();
        ok(&json!({"count": rows[0].int(0)}))
    })
}

fn count_schema() -> Value {
    json!({"type": "object"})
}

struct NotesTasks;

impl Tasks for NotesTasks {
    fn create<'a>(
        &'a self,
        _user: &'a str,
        title: &'a str,
        _assignee: &'a str,
        _due_ms: Option<i64>,
        _from: Option<ItemRef>,
    ) -> BoxFuture<'a, Result<ItemRef, String>> {
        Box::pin(async move {
            Ok(ItemRef {
                app: "notes".into(),
                id: title.into(),
                path: format!("/notes/{title}"),
            })
        })
    }
}

fn ticker(_: Arc<Instance>, mut stop: Stop) -> BoxFuture<'static, Result<(), String>> {
    Box::pin(async move {
        loop {
            TICKS.fetch_add(1, Ordering::SeqCst);
            tokio::select! {
                _ = stop.changed() => return Ok(()),
                () = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
    })
}

impl App for Notes {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "notes",
            group: Group::Create,
            path: "/notes",
            order: 1,
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        MIGRATIONS
    }

    fn routes(&self) -> Vec<Route> {
        vec![Route {
            method: Method::Get,
            path: "/notes/count",
            id: "countNotes",
            tag: "notes",
            summary: "Count the notes.",
            response: count_schema,
            handler: count,
        }]
    }

    fn jobs(&self, _: &Arc<Instance>) -> Vec<JobKind> {
        vec![JobKind {
            kind: "notes.add".into(),
            handler: Box::new(|job, tx| {
                Box::pin(async move {
                    tx.execute(
                        "INSERT INTO notes_items (id, body) VALUES ($1, $2)",
                        &[job.id.as_str().into(), job.payload.as_str().into()],
                    )
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                })
            }),
        }]
    }

    fn role_tasks(&self) -> Vec<RoleTask> {
        vec![RoleTask {
            role: Role::Media,
            name: "ticker",
            run: ticker,
        }]
    }

    fn provide(&self, _: &Arc<Instance>, services: &Services) {
        services.provide::<dyn Tasks>("notes", Arc::new(NotesTasks));
    }
}

/// A second app that only lists itself.
struct Chat;

impl App for Chat {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "chat",
            group: Group::Communicate,
            path: "/chat",
            order: 1,
        }
    }
}

fn apps() -> Vec<Arc<dyn App>> {
    vec![Arc::new(Notes), Arc::new(Chat)]
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

fn listed(instance: &Arc<Instance>) -> Vec<String> {
    let client = TestClient::new(ortus_core::app(Arc::clone(instance)));
    let body = json_of(client.get("/api/v1/apps").send().body_text());
    body["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|app| {
            format!(
                "{}:{}:{}",
                app["id"].as_str().unwrap(),
                app["group"].as_str().unwrap(),
                app["path"].as_str().unwrap()
            )
        })
        .collect()
}

#[test]
fn installed_apps_are_listed_in_switcher_order() {
    let instance = testing::instance(apps());
    assert_eq!(
        vec!["chat:communicate:/chat", "notes:create:/notes"],
        listed(&instance)
    );
}

#[test]
fn an_app_answers_its_routes_and_screens() {
    let instance = testing::instance(apps());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let response = client.get("/api/v1/notes/count").send();
    assert_eq!(200, response.status(), "{}", response.body_text());
    assert_eq!(json!({"count": 0}), json_of(response.body_text()));
    for screen in [
        "/notes",
        "/notes/some/note",
        "/chat",
        "/welcome",
        "/admin/users",
    ] {
        let response = client.get(screen).send();
        assert_eq!(200, response.status(), "{screen}");
        assert!(response.body_text().contains("<html"), "{screen}");
    }
    for path in ["/notesx", "/other", "/api/v1/notes/nothing"] {
        assert_eq!(404, client.get(path).send().status(), "{path}");
    }
}

#[test]
fn a_switched_off_app_is_gone_but_keeps_its_data() {
    let instance = testing::instance(apps());
    block_on(instance.store.execute(
        "INSERT INTO notes_items (id, body) VALUES ('n1', 'kept')",
        &[],
    ))
    .unwrap();
    block_on(instance.switches.set("notes", false, 1)).unwrap();
    assert_eq!(vec!["chat:communicate:/chat"], listed(&instance));

    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let response = client.get("/api/v1/notes/count").send();
    assert_eq!(404, response.status());
    let body = json_of(response.body_text());
    assert_eq!("not_installed", body["error"]["code"]);
    assert!(body["error"]["message"].as_str().unwrap().contains("notes"));
    assert!(matches!(
        block_on(instance.service::<dyn Tasks>()),
        Err(ortus_core::services::NotInstalled { .. })
    ));

    block_on(instance.switches.set("notes", true, 2)).unwrap();
    let response = client.get("/api/v1/notes/count").send();
    assert_eq!(json!({"count": 1}), json_of(response.body_text()));
}

#[test]
fn the_configuration_can_start_an_app_switched_off() {
    let instance = testing::instance(apps());
    let config = Config::load(Some("[apps]\noff = [\"notes\"]"), &|_| None).unwrap();
    let off = Instance::assemble(
        config,
        Arc::clone(&instance.store),
        Arc::clone(&instance.files),
        apps(),
    );
    assert_eq!(vec!["chat:communicate:/chat"], listed(&off));
    // An administrator's switch wins over the configuration.
    block_on(off.switches.set("notes", true, 1)).unwrap();
    assert_eq!(
        vec!["chat:communicate:/chat", "notes:create:/notes"],
        listed(&off)
    );
}

#[test]
fn a_service_is_found_by_its_trait_or_not_installed() {
    let instance = testing::instance(apps());
    let tasks = block_on(instance.service::<dyn Tasks>()).unwrap();
    let made = block_on(tasks.create("olena", "close-r12", "marek", None, None)).unwrap();
    assert_eq!("/notes/close-r12", made.path);
    assert_eq!(Some("notes"), instance.services.provider::<dyn Tasks>());

    let without = testing::instance(vec![Arc::new(Chat)]);
    let answer = block_on(without.service::<dyn Tasks>());
    let Err(not_installed) = answer else {
        panic!("no app provides Tasks here");
    };
    assert!(not_installed.service.contains("Tasks"), "{not_installed}");
}

#[test]
fn jobs_of_a_switched_off_app_wait_in_the_queue() {
    let instance = testing::instance(apps());
    let store = instance.store.as_ref();
    let job = NewJob {
        id: "add-1",
        kind: "notes.add",
        payload: "from a job",
        run_at: 0,
        max_attempts: 3,
    };
    block_on(jobs::enqueue(store, &job, 0)).unwrap();
    block_on(instance.switches.set("notes", false, 1)).unwrap();
    let worker = AppWorker::new(Arc::clone(&instance), "test-worker");
    assert!(
        !block_on(worker.tick(10)).unwrap(),
        "nothing may run while notes is off"
    );
    assert_eq!(
        Some(jobs::JobState::Queued),
        block_on(jobs::state(store, "add-1")).unwrap()
    );

    block_on(instance.switches.set("notes", true, 2)).unwrap();
    assert!(block_on(worker.tick(20)).unwrap());
    assert_eq!(
        Some(jobs::JobState::Done),
        block_on(jobs::state(store, "add-1")).unwrap()
    );
    let rows =
        block_on(store.query("SELECT body FROM notes_items WHERE id = 'add-1'", &[])).unwrap();
    assert_eq!(Some("from a job"), rows[0].text(0));
}

#[test]
fn role_tasks_run_for_their_role_only_and_stop_with_the_program() {
    // Runs the program with `roles` until the ticker has ticked three times
    // or `wait` has passed, and returns the ticks.
    let run = |roles: &str, wait: Duration| {
        TICKS.store(0, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("ortus-core-apps-{}-{roles}", std::process::id()));
        let roles = roles.to_string();
        let data = dir.to_string_lossy().into_owned();
        let config = Config::load(None, &move |name| match name {
            "ORTUS_ROLES" => Some(roles.clone()),
            "ORTUS_DATA_DIR" => Some(data.clone()),
            _ => None,
        })
        .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(async {
                let stop = async move {
                    let deadline = tokio::time::Instant::now() + wait;
                    while TICKS.load(Ordering::SeqCst) < 3 && tokio::time::Instant::now() < deadline
                    {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                };
                ortus_core::program::run_until(config, apps(), stop).await
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        TICKS.load(Ordering::SeqCst)
    };
    assert!(
        run("media", Duration::from_secs(10)) >= 3,
        "the media role runs the ticker"
    );
    assert_eq!(
        0,
        run("worker", Duration::from_millis(500)),
        "the worker role does not"
    );
}

#[test]
fn apps_that_do_not_fit_are_refused() {
    struct Bad(AppInfo, Vec<Route>);
    impl App for Bad {
        fn info(&self) -> AppInfo {
            self.0
        }
        fn routes(&self) -> Vec<Route> {
            self.1.clone()
        }
    }
    let info = |id: &'static str, path: &'static str| AppInfo {
        id,
        group: Group::Create,
        path,
        order: 0,
    };
    let route = Notes.routes()[0];
    let cases: Vec<(Vec<Arc<dyn App>>, &str)> = vec![
        (
            vec![Arc::new(Bad(info("Notes", "/Notes"), vec![]))],
            "lowercase",
        ),
        (
            vec![Arc::new(Bad(info("admin", "/admin"), vec![]))],
            "reserved",
        ),
        (vec![Arc::new(Notes), Arc::new(Notes)], "two apps"),
        (
            vec![Arc::new(Bad(info("drive", "/files"), vec![]))],
            "must live at",
        ),
        (
            vec![Arc::new(Bad(info("drive", "/drive"), vec![route]))],
            "outside /drive",
        ),
    ];
    for (apps, why) in cases {
        let error = app::check(&apps).unwrap_err();
        assert!(error.contains(why), "{error}");
    }
    assert!(app::check(&apps()).is_ok());
}
