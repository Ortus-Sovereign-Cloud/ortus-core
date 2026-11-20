//! What one app may ask of another. Each trait is provided by at most one
//! app and asked for through [`crate::services::Services`]; when its app
//! is absent or off, the asker gets [`crate::services::NotInstalled`].
//!
//! The traits grow with the apps that need them. Every method is async and
//! names the person it acts for, so the provider applies that person's
//! permissions and labels.

use crate::storage::BoxFuture;

/// A person, by their account id.
pub type UserId = String;

/// Something another app keeps, by the app and its own id for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemRef {
    pub app: String,
    pub id: String,
    /// Where it opens in the web client.
    pub path: String,
}

/// Provided by the calendar app; used by mail for invitations.
pub trait Calendar: Send + Sync {
    /// Puts the invitation (iCalendar text) into the person's calendar.
    fn add_invitation<'a>(
        &'a self,
        user: &'a str,
        ics: &'a str,
    ) -> BoxFuture<'a, Result<ItemRef, String>>;
}

/// Provided by the contacts part of the calendar app; used by mail and
/// chat to suggest people.
pub trait Contacts: Send + Sync {
    /// People and addresses that match `query`, best first.
    fn suggest<'a>(
        &'a self,
        user: &'a str,
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<Suggestion>, String>>;
}

/// One suggested person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub name: String,
    pub address: String,
}

/// Provided by the drive app; used by mail and chat to keep attachments.
pub trait Drive: Send + Sync {
    /// Saves a file into the person's drive.
    fn save<'a>(
        &'a self,
        user: &'a str,
        name: &'a str,
        contents: &'a [u8],
    ) -> BoxFuture<'a, Result<ItemRef, String>>;
}

/// Provided by the chat app; used by tasks, approvals and records to tell
/// a channel what happened.
pub trait Chat: Send + Sync {
    /// Posts a message to a channel as the person.
    fn post<'a>(
        &'a self,
        user: &'a str,
        channel: &'a str,
        text: &'a str,
    ) -> BoxFuture<'a, Result<ItemRef, String>>;
}

/// Provided by the tasks app; used by chat to turn a decision into a task.
pub trait Tasks: Send + Sync {
    /// Creates a task for `assignee`, due at `due_ms` (Unix milliseconds).
    fn create<'a>(
        &'a self,
        user: &'a str,
        title: &'a str,
        assignee: &'a str,
        due_ms: Option<i64>,
        from: Option<ItemRef>,
    ) -> BoxFuture<'a, Result<ItemRef, String>>;
}
