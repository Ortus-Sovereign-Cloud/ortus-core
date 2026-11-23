//! Changes worth waking up for: live streams (chat, documents) wait on a
//! topic instead of asking the database every few hundred milliseconds.
//! In one program a change wakes its waiters directly ([`wake_soon`]);
//! with PostgreSQL, a trigger's `pg_notify('ortus_changes', topic)` reaches
//! every program through the one connection each keeps listening
//! ([`crate::storage::postgres`]). Waiters still look on their own every
//! few seconds, so a lost signal only delays.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

static TOPICS: LazyLock<Mutex<HashMap<String, Arc<Notify>>>> = LazyLock::new(Default::default);

fn notifier(topic: &str) -> Arc<Notify> {
    let mut all = TOPICS.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(all.entry(topic.to_string()).or_default())
}

/// Wakes everyone waiting on `topic` now.
pub fn wake(topic: &str) {
    notifier(topic).notify_waiters();
}

/// Wakes everyone waiting on `topic` a moment from now, after the
/// transaction that made the change has committed.
pub fn wake_soon(topic: &str) {
    let topic = topic.to_string();
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            wake(&topic);
        });
    } else {
        wake(&topic);
    }
}

/// Waits until `topic` changes, or `at_most` has passed.
pub async fn wait(topic: &str, at_most: Duration) {
    let notify = notifier(topic);
    let _ = tokio::time::timeout(at_most, notify.notified()).await;
}

/// The PostgreSQL statement a trigger function runs to tell every program
/// that `topic` changed (delivered when its transaction commits).
pub fn notify_sql(topic: &str) -> String {
    format!(
        "PERFORM pg_notify('ortus_changes', '{}');",
        topic.replace('\'', "''")
    )
}
