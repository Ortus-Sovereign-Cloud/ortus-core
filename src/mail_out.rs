//! Mail from Ortus itself: sign-up codes, invitations and notices, handed
//! to the organisation's mail server by SMTP submission. (The mail app has
//! its own sending; this is for the core.)
//!
//! Messages go through the job queue, so a mail server that is down for a
//! while delays them instead of losing them. A job that is retried after a
//! crash may send a message twice; these messages are safe to repeat.

use std::sync::Arc;

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::JobKind;
use crate::config::{MailSecurity, OutgoingMail};
use crate::jobs::{self, NewJob};
use crate::storage::{AsyncStore, StoreResult};

/// The kind of job that sends one message.
pub const JOB: &str = "core.mail";

/// A plain-text message to one person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outgoing {
    pub to: String,
    pub subject: String,
    pub text: String,
}

/// Sends `message` now.
pub async fn send(settings: &OutgoingMail, message: &Outgoing) -> Result<(), String> {
    let email = lettre::Message::builder()
        .from(
            settings
                .from
                .parse()
                .map_err(|e| format!("outgoing_mail.from: {e}"))?,
        )
        .to(message
            .to
            .parse()
            .map_err(|e| format!("{}: {e}", message.to))?)
        .subject(message.subject.clone())
        .header(ContentType::TEXT_PLAIN)
        .body(message.text.clone())
        .map_err(|e| e.to_string())?;
    let builder = match settings.security {
        MailSecurity::StartTls => {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&settings.host)
                .map_err(|e| e.to_string())?
        }
        MailSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&settings.host)
            .map_err(|e| e.to_string())?,
        MailSecurity::None => {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&settings.host)
        }
    };
    let mut builder = builder
        .port(settings.port)
        .timeout(Some(std::time::Duration::from_secs(30)));
    if let (Some(user), Some(password)) = (&settings.username, &settings.password) {
        builder = builder.credentials(Credentials::new(
            user.clone(),
            password.as_str().to_string(),
        ));
    }
    builder
        .build()
        .send(email)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Queues `message` to be sent by a worker. The same message queued twice
/// in one minute is sent once.
pub async fn queue(store: &dyn AsyncStore, message: &Outgoing, now_ms: i64) -> StoreResult<()> {
    let payload = serde_json::to_string(message).unwrap_or_default();
    let id = format!(
        "mail-{}-{}",
        now_ms / 60_000,
        hex::encode(&Sha256::digest(payload.as_bytes())[..12])
    );
    jobs::enqueue(
        store,
        &NewJob {
            id: &id,
            kind: JOB,
            payload: &payload,
            run_at: now_ms,
            max_attempts: 8,
        },
        now_ms,
    )
    .await?;
    Ok(())
}

/// The job that sends queued messages, when outgoing mail is set up.
pub fn job(settings: Option<OutgoingMail>) -> Option<JobKind> {
    let settings = Arc::new(settings?);
    Some(JobKind {
        kind: JOB.into(),
        handler: Box::new(move |job, _tx| {
            let settings = Arc::clone(&settings);
            let payload = job.payload.clone();
            Box::pin(async move {
                let message: Outgoing =
                    serde_json::from_str(&payload).map_err(|e| format!("bad mail job: {e}"))?;
                send(&settings, &message).await
            })
        }),
    })
}
