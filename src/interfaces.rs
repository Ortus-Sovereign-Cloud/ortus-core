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

/// Provided by the calendar app; used by mail for invitations and the
/// day's agenda, and by compose for times that work.
pub trait Calendar: Send + Sync {
    /// An iMIP message (RFC 6047) came for the person from `sender` (the
    /// address mail verified): a REQUEST goes into their calendar, a REPLY
    /// updates the event they organise, a CANCEL cancels it.
    fn receive<'a>(
        &'a self,
        user: &'a str,
        sender: &'a str,
        ics: &'a str,
    ) -> BoxFuture<'a, Result<Invitation, String>>;

    /// The person's copy of the event with this UID, as an invitation
    /// (their answer, when, whether it is cancelled), if they have it.
    fn invitation<'a>(
        &'a self,
        user: &'a str,
        uid: &'a str,
    ) -> BoxFuture<'a, Result<Option<Invitation>, String>>;

    /// The person answers an invitation: ACCEPTED, TENTATIVE or DECLINED.
    fn respond<'a>(
        &'a self,
        user: &'a str,
        uid: &'a str,
        answer: &'a str,
    ) -> BoxFuture<'a, Result<Invitation, String>>;

    /// The person's events between two times (Unix ms), in order.
    fn agenda<'a>(
        &'a self,
        user: &'a str,
        from_ms: i64,
        to_ms: i64,
    ) -> BoxFuture<'a, Result<Vec<AgendaItem>, String>>;

    /// When everyone in `people` (addresses here) is free for `minutes`
    /// between two times: up to `limit` starts (Unix ms).
    fn free_times<'a>(
        &'a self,
        user: &'a str,
        people: &'a [String],
        from_ms: i64,
        to_ms: i64,
        minutes: i64,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<i64>, String>>;
}

/// An invitation as the person sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invitation {
    pub uid: String,
    /// REQUEST, REPLY or CANCEL.
    pub method: String,
    pub summary: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub organizer: Option<String>,
    /// The person's own answer: NEEDS-ACTION, ACCEPTED, TENTATIVE, DECLINED.
    pub answer: Option<String>,
    pub cancelled: bool,
    pub item: Option<ItemRef>,
}

/// One event in the day's agenda.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgendaItem {
    pub uid: String,
    pub summary: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub all_day: bool,
    /// The person's answer when it is an invitation.
    pub answer: Option<String>,
    pub item: ItemRef,
}

/// Provided by the mail app; used by the calendar to send invitations
/// and answers to people outside (iMIP), past the label policy.
pub trait Mailer: Send + Sync {
    /// Sends `ics` (with its METHOD) from the person to `to`, with a plain
    /// text for mail apps that do not read calendars.
    fn send_invitation<'a>(
        &'a self,
        user: &'a str,
        to: &'a [String],
        subject: &'a str,
        text: &'a str,
        ics: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
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
    /// Saves a file into the person's drive. `source` says which app it
    /// came from ("mail"), `note` how ("Attached by Anna Kowalska").
    fn save<'a>(
        &'a self,
        user: &'a str,
        name: &'a str,
        mime: &'a str,
        contents: &'a [u8],
        source: &'a str,
        note: Option<&'a str>,
    ) -> BoxFuture<'a, Result<ItemRef, String>>;

    /// Makes an item whose contents another app keeps (a document of the
    /// docs app): it lives in the person's drive, in `parent` ("" for the
    /// root), with Drive's sharing and labels.
    fn create<'a>(
        &'a self,
        user: &'a str,
        parent: &'a str,
        name: &'a str,
        mime: &'a str,
    ) -> BoxFuture<'a, Result<ItemRef, String>>;

    /// What the person may do with an item and how it is labelled, or
    /// `None` when they cannot see it.
    fn describe<'a>(
        &'a self,
        user: &'a str,
        id: &'a str,
    ) -> BoxFuture<'a, Result<Option<DriveItem>, String>>;

    /// Renames an item (the person must be able to edit it).
    fn rename<'a>(
        &'a self,
        user: &'a str,
        id: &'a str,
        name: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
}

/// An item of the drive, as another app sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveItem {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub owner: String,
    /// view, comment, edit or owner.
    pub role: String,
    /// The label that governs it (its own or the strictest above).
    pub label: Option<String>,
    /// The folders above it the person may see, top first.
    pub path: Vec<String>,
    pub modified_at: i64,
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
