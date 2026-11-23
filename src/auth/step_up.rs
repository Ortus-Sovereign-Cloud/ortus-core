//! Step-up: what would let a stolen session outlast itself (a phone
//! sign-in link, new recovery codes, removing the authenticator, a new
//! passkey) or put the person's name on something (signing) needs a
//! recent, strong proof that it is them: a session begun in the last ten
//! minutes with a passkey, a password and code, or the organisation's own
//! sign-in; or a confirmation in that time (`POST /session/confirm`, with a
//! passkey or the password and code). A session from a phone link, a
//! recovery code, an invitation or a mailed link is not proof enough.

use crate::api::ApiError;
use crate::auth::sessions::Session;
use crate::program::now_ms;
use crate::storage::{AsyncStore, StoreResult};

/// How long a strong sign-in or a confirmation counts.
pub const WINDOW_MS: i64 = 10 * 60 * 1000;

/// Whether a way of signing in proves who someone is.
pub fn strong(methods: &str) -> bool {
    methods == "passkey"
        || methods == "setup"
        || methods.ends_with("+totp")
        || methods.starts_with("external:")
        || methods.starts_with("saml:")
}

/// Whether the session has proved, recently, that it is its person.
pub async fn confirmed(store: &dyn AsyncStore, session: &Session) -> StoreResult<bool> {
    let rows = store
        .query(
            "SELECT confirmed_at FROM core_sessions WHERE token_hash = $1",
            &[session.token_hash.as_str().into()],
        )
        .await?;
    let confirmed_at = rows.first().and_then(|r| r.int(0)).unwrap_or(0);
    let began = if strong(&session.methods) {
        session.created_at
    } else {
        0
    };
    Ok(now_ms() - began.max(confirmed_at) < WINDOW_MS)
}

/// Refuses with `step_up` (403) unless the session proved it recently;
/// the client then confirms and tries again.
pub async fn require(store: &dyn AsyncStore, session: &Session) -> Result<(), ApiError> {
    if confirmed(store, session).await? {
        return Ok(());
    }
    Err(ApiError {
        status: 403,
        code: "step_up",
        message: "Confirm it is you first: use your passkey, or your password and code.".into(),
    })
}

/// Records a confirmation on the session.
pub async fn record(store: &dyn AsyncStore, session: &Session) -> StoreResult<()> {
    store
        .execute(
            "UPDATE core_sessions SET confirmed_at = $1 WHERE token_hash = $2",
            &[now_ms().into(), session.token_hash.as_str().into()],
        )
        .await?;
    Ok(())
}
