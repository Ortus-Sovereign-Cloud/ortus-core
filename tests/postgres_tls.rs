//! TLS to PostgreSQL, against a server with its own CA (started by
//! ci/postgres-tls.sh). Runs when ORTUS_TEST_POSTGRES_TLS is set; CI sets
//! ORTUS_REQUIRE_POSTGRES_TLS=1 so a missing server fails there.

use ortus_core::storage::blocking::on;
use ortus_core::storage::PostgresStore;

fn server() -> Option<(String, String)> {
    let Ok(url) = std::env::var("ORTUS_TEST_POSTGRES_TLS") else {
        assert!(
            std::env::var("ORTUS_REQUIRE_POSTGRES_TLS").is_err(),
            "ORTUS_REQUIRE_POSTGRES_TLS is set but ORTUS_TEST_POSTGRES_TLS is not"
        );
        eprintln!("skipped: run ci/postgres-tls.sh and export what it prints");
        return None;
    };
    Some((
        url,
        std::env::var("ORTUS_TEST_POSTGRES_TLS_CA").expect("ORTUS_TEST_POSTGRES_TLS_CA"),
    ))
}

fn encrypted(url: &str) -> Result<bool, String> {
    let store = PostgresStore::connect(url, 1).map_err(|e| e.to_string())?;
    let rows = on(&store)
        .query(
            "SELECT CASE WHEN ssl THEN 1 ELSE 0 END FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            &[],
        )
        .map_err(|e| e.to_string())?;
    Ok(rows.first().and_then(|r| r.int(0)) == Some(1))
}

#[test]
fn connections_are_encrypted_as_the_url_says() {
    let Some((url, ca)) = server() else { return };
    assert_eq!(
        Ok(true),
        encrypted(&url),
        "the default encrypts when the server offers it"
    );
    assert_eq!(Ok(true), encrypted(&format!("{url}?sslmode=require")));
    assert_eq!(Ok(false), encrypted(&format!("{url}?sslmode=disable")));
    assert_eq!(
        Ok(true),
        encrypted(&format!("{url}?sslmode=verify-full&sslrootcert={ca}"))
    );
    // A certificate from an unknown CA is refused when it must be checked.
    assert!(encrypted(&format!("{url}?sslmode=verify-full")).is_err());
}
