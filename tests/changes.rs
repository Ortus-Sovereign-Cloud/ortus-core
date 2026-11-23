//! Review A6: waiting on a topic ends when it changes, in this program
//! (wake) and, with PostgreSQL, when any program's transaction notifies it
//! (pg_notify, delivered on commit) — or at the latest after the time given.

mod common;

use std::time::{Duration, Instant};

use ortus_core::changes;
use ortus_core::runtime::block_on;

#[test]
fn a_wait_ends_when_its_topic_changes() {
    block_on(async {
        // Nothing happens: the wait ends at its limit.
        let started = Instant::now();
        changes::wait("quiet-topic", Duration::from_millis(200)).await;
        assert!(started.elapsed() >= Duration::from_millis(190));
        // A change in this program ends it at once.
        let started = Instant::now();
        let waiting = tokio::spawn(changes::wait("local-topic", Duration::from_secs(5)));
        tokio::time::sleep(Duration::from_millis(50)).await;
        changes::wake_soon("local-topic");
        waiting.await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    });
}

#[test]
fn a_commit_on_postgres_wakes_every_program() {
    let Some(fresh) = common::postgres() else {
        return;
    };
    let store = (fresh.open)();
    block_on(async {
        // The listening connection starts with the store.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let started = Instant::now();
        let waiting = tokio::spawn(changes::wait("chat-test", Duration::from_secs(10)));
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut tx = store.begin().await.unwrap();
        tx.query("SELECT pg_notify('ortus_changes', 'chat-test')", &[])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        waiting.await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    });
}
