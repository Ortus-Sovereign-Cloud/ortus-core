//! What one API request carries while it is served: the session it was
//! signed in with (looked up once, however often the request asks), and
//! what it spent on the database.
//!
//! With `ORTUS_SLOW_MS` set, a request that takes longer than that many
//! milliseconds is logged with its database statements, the time they
//! took and the time spent waiting for a free connection.

use std::cell::RefCell;
use std::future::Future;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::auth::sessions::Session;

#[derive(Default)]
struct State {
    /// The token asked about and what it gave, for this request.
    session: Option<(String, Option<Session>)>,
    statements: u32,
    database: Duration,
    waited: Duration,
    opened: u32,
}

tokio::task_local! {
    static REQUEST: RefCell<State>;
}

fn slow_ms() -> Option<u128> {
    static SLOW: OnceLock<Option<u128>> = OnceLock::new();
    *SLOW.get_or_init(|| {
        std::env::var("ORTUS_SLOW_MS")
            .ok()
            .and_then(|v| v.parse().ok())
    })
}

/// What a request cost in the database.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    pub statements: u32,
    pub database: Duration,
    pub waited: Duration,
}

impl Cost {
    /// The `Server-Timing` header that says it, when `ORTUS_SERVER_TIMING`
    /// is set (for measuring; off by default).
    pub fn server_timing(&self) -> Option<String> {
        static ON: OnceLock<bool> = OnceLock::new();
        ON.get_or_init(|| std::env::var_os("ORTUS_SERVER_TIMING").is_some())
            .then(|| {
                format!(
                    "db;dur={:.1};desc=\"{} statements\", wait;dur={:.1}",
                    self.database.as_secs_f64() * 1000.0,
                    self.statements,
                    self.waited.as_secs_f64() * 1000.0
                )
            })
    }
}

/// Serves `future` as one request: `what` names it in the slow log.
pub async fn serve<F: Future>(what: impl FnOnce() -> String, future: F) -> (F::Output, Cost) {
    let started = Instant::now();
    let (output, state) = REQUEST
        .scope(RefCell::new(State::default()), async move {
            let output = future.await;
            let state = REQUEST.with(|s| std::mem::take(&mut *s.borrow_mut()));
            (output, state)
        })
        .await;
    if let Some(limit) = slow_ms() {
        let took = started.elapsed();
        if took.as_millis() >= limit {
            eprintln!(
                "ortus: slow: {} {} ms: {} statements, {} ms in the database, {} ms waiting for a connection, {} connections opened",
                what(),
                took.as_millis(),
                state.statements,
                state.database.as_millis(),
                state.waited.as_millis(),
                state.opened
            );
        }
    }
    let cost = Cost {
        statements: state.statements,
        database: state.database,
        waited: state.waited,
    };
    (output, cost)
}

/// The session this request already found for `token`.
pub(crate) fn session(token: &str) -> Option<Option<Session>> {
    REQUEST
        .try_with(|s| {
            s.borrow()
                .session
                .as_ref()
                .filter(|(t, _)| t == token)
                .map(|(_, found)| found.clone())
        })
        .ok()
        .flatten()
}

/// Remembers the session found for `token` for the rest of the request.
pub(crate) fn remember_session(token: &str, found: &Option<Session>) {
    let _ = REQUEST.try_with(|s| s.borrow_mut().session = Some((token.to_string(), found.clone())));
}

/// Forgets the session (it was ended, or its person changed).
pub(crate) fn forget_session() {
    let _ = REQUEST.try_with(|s| s.borrow_mut().session = None);
}

/// One database statement, and how long it took.
pub(crate) fn statement(took: Duration) {
    let _ = REQUEST.try_with(|s| {
        let mut s = s.borrow_mut();
        s.statements += 1;
        s.database += took;
    });
}

/// Time spent waiting for a free database connection.
pub(crate) fn waited(took: Duration) {
    let _ = REQUEST.try_with(|s| s.borrow_mut().waited += took);
}

/// A new database connection was opened.
pub(crate) fn opened() {
    let _ = REQUEST.try_with(|s| s.borrow_mut().opened += 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn counts_belong_to_their_request() {
        statement(Duration::from_millis(5));
        let (((), a), ((), b)) = tokio::join!(
            serve(String::new, async {
                statement(Duration::from_millis(1));
                statement(Duration::from_millis(1));
                assert_eq!(2, REQUEST.with(|s| s.borrow().statements));
            }),
            serve(String::new, async {
                statement(Duration::from_millis(1));
                assert_eq!(1, REQUEST.with(|s| s.borrow().statements));
            })
        );
        assert_eq!((2, 1), (a.statements, b.statements));
        assert!(session("t").is_none());
    }
}
