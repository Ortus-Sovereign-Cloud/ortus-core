//! Profile pictures: a person's own photo (PNG, JPEG or WebP, checked by
//! their bytes, never SVG; small, as the client crops and shrinks it), or
//! one from the organisation's directory. Anyone signed in sees a
//! colleague's photo, by account or by address; an administrator can turn
//! uploads off for the installation and remove a photo.

use std::sync::Arc;

use rust_web_server::header::Header;
use rust_web_server::prelude::*;
use rust_web_server::range::Range;
use serde::Deserialize;
use serde_json::{json, Value};

use super::accounts::{self, User};
use super::Access;
use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture, Migration, Schema, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "profile pictures",
    sql: "CREATE TABLE core_photos (
        user_id TEXT PRIMARY KEY REFERENCES core_users (id),
        mime TEXT NOT NULL,
        source TEXT NOT NULL,
        updated_at BIGINT NOT NULL
    )",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.photos",
    migrations: MIGRATIONS,
};

/// The largest photo taken (the client sends 256 by 256 pixels).
pub const MAX_BYTES: usize = 512 * 1024;
/// The installation-wide setting that turns uploads off.
const UPLOADS: &str = "photo_uploads";

fn key(user_id: &str) -> String {
    format!("core/photos/{user_id}")
}

/// The image type of `bytes`, by their first bytes: PNG, JPEG or WebP.
pub fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Whether people may upload their own photo here.
pub async fn uploads_allowed(store: &dyn AsyncStore) -> StoreResult<bool> {
    Ok(super::policy::setting(store, UPLOADS).await?.as_deref() != Some("off"))
}

/// Keeps `bytes` as the person's photo, from `source` ("own" or
/// "directory").
pub async fn keep(
    ortus: &Instance,
    user_id: &str,
    bytes: &[u8],
    source: &str,
) -> Result<(), ApiError> {
    if bytes.len() > MAX_BYTES {
        return Err(ApiError {
            status: 413,
            code: "too_large",
            message: format!("A photo is at most {} KB.", MAX_BYTES / 1024),
        });
    }
    let mime = image_type(bytes)
        .ok_or_else(|| ApiError::bad_request("A photo is a PNG, JPEG or WebP image."))?;
    ortus
        .files
        .put(&key(user_id), bytes)
        .await
        .map_err(|_| ApiError::internal())?;
    let store = ortus.store.as_ref();
    let mut tx = store.begin().await?;
    tx.execute(
        "DELETE FROM core_photos WHERE user_id = $1",
        &[user_id.into()],
    )
    .await?;
    tx.execute(
        "INSERT INTO core_photos (user_id, mime, source, updated_at) VALUES ($1, $2, $3, $4)",
        &[user_id.into(), mime.into(), source.into(), now_ms().into()],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The source of the person's photo ("own", "directory"), if they have one.
pub async fn source(store: &dyn AsyncStore, user_id: &str) -> StoreResult<Option<String>> {
    Ok(store
        .query(
            "SELECT source FROM core_photos WHERE user_id = $1",
            &[user_id.into()],
        )
        .await?
        .first()
        .and_then(|r| r.text(0))
        .map(str::to_string))
}

async fn forget(ortus: &Instance, user_id: &str) -> Result<(), ApiError> {
    ortus
        .store
        .execute(
            "DELETE FROM core_photos WHERE user_id = $1",
            &[user_id.into()],
        )
        .await?;
    let _ = ortus.files.delete(&key(user_id)).await;
    Ok(())
}

fn header(name: &str, value: &str) -> Header {
    Header {
        name: name.to_string().into(),
        value: value.to_string().into(),
    }
}

/// A person's photo, or 204 No Content when there is none: the same for an
/// unknown account (nothing to learn from it), and quiet in browsers, which
/// show initials instead (a 404 for every colleague without a photo would
/// fill the console).
async fn photo_of(ortus: &Instance, user: Option<User>) -> Result<Response, ApiError> {
    match photo_bytes(ortus, user).await? {
        Some(response) => Ok(response),
        None => Ok(Response::get_response(
            STATUS_CODE_REASON_PHRASE.n204_no_content,
            Some(vec![header("Cache-Control", "private, max-age=300")]),
            None,
        )),
    }
}

async fn photo_bytes(ortus: &Instance, user: Option<User>) -> Result<Option<Response>, ApiError> {
    let Some(user) = user else { return Ok(None) };
    let rows = ortus
        .store
        .query(
            "SELECT mime, updated_at FROM core_photos WHERE user_id = $1",
            &[user.id.as_str().into()],
        )
        .await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let mime = row.text(0).unwrap_or("image/png").to_string();
    let bytes = ortus
        .files
        .get(&key(&user.id))
        .await
        .map_err(|_| ApiError::internal())?;
    let Some(bytes) = bytes else { return Ok(None) };
    // Only the image types taken; anything else is not served.
    if image_type(&bytes) != Some(mime.as_str()) {
        return Ok(None);
    }
    let headers = vec![
        // A changed photo shows within minutes; ?v= in a link shows it at once.
        header("Cache-Control", "private, max-age=300"),
        header("X-Content-Type-Options", "nosniff"),
        header("Content-Disposition", "inline"),
        header("Content-Security-Policy", "default-src 'none'"),
    ];
    Ok(Some(Response::get_response(
        STATUS_CODE_REASON_PHRASE.n200_ok,
        Some(headers),
        Some(vec![Range::get_content_range(bytes, mime)]),
    )))
}

async fn by_id(
    _: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let id = params.get("id").unwrap_or_default();
    let user = accounts::by_id(ortus.store.as_ref(), id).await?;
    photo_of(ortus, user).await
}

async fn by_address(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let email = crate::form::query(&request.request_uri, "email").unwrap_or_default();
    let user = match accounts::normal_email(&email) {
        Some(email) => accounts::by_email(ortus.store.as_ref(), &email).await?,
        None => None,
    };
    photo_of(ortus, user).await
}

async fn me(request: &Request, ortus: &Arc<Instance>) -> Result<User, ApiError> {
    Ok(super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?
        .user)
}

/// Whether the person has a photo and may upload one.
async fn mine(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = me(request, ortus).await?;
    let store = ortus.store.as_ref();
    Ok(ok(&json!({
        "source": source(store, &me.id).await?,
        "uploads_allowed": uploads_allowed(store).await?,
        "max_bytes": MAX_BYTES,
    })))
}

async fn upload(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = me(request, ortus).await?;
    if !uploads_allowed(ortus.store.as_ref()).await? {
        return Err(ApiError {
            status: 403,
            code: "photos_off",
            message: "Photos are turned off here.".into(),
        });
    }
    keep(ortus, &me.id, request.body.as_bytes(), "own").await?;
    super::audit(
        ortus,
        crate::audit::Event::new("people.photo_set", crate::audit::Outcome::Done)
            .by(&me.id)
            .on(&me.id),
    )
    .await;
    Ok(ok(&json!({ "source": "own" })))
}

async fn remove_mine(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = me(request, ortus).await?;
    forget(ortus, &me.id).await?;
    Ok(ok(&json!({ "source": null })))
}

/// An administrator removes someone's photo (one that should not be there).
async fn remove(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = me(request, ortus).await?;
    let id = params.get("id").unwrap_or_default().to_string();
    let user = accounts::by_id(ortus.store.as_ref(), &id)
        .await?
        .ok_or_else(|| ApiError::not_found("There is no such person."))?;
    if user.role > me.role {
        return Err(ApiError::forbidden());
    }
    forget(ortus, &id).await?;
    super::audit(
        ortus,
        crate::audit::Event::new("people.photo_removed", crate::audit::Outcome::Done)
            .by(&me.id)
            .on(&id),
    )
    .await;
    Ok(ok(&json!({ "removed": id })))
}

#[derive(Deserialize)]
struct Uploads {
    allowed: bool,
}

/// Turns people's own uploads on or off (directory photos stay).
async fn set_uploads(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = me(request, ortus).await?;
    let input: Uploads = body(request)?;
    super::policy::set_setting(
        ortus.store.as_ref(),
        UPLOADS,
        if input.allowed { "on" } else { "off" },
    )
    .await?;
    super::audit(
        ortus,
        crate::audit::Event::new("people.photo_uploads", crate::audit::Outcome::Done)
            .by(&me.id)
            .with(json!({ "allowed": input.allowed })),
    )
    .await;
    Ok(ok(&json!({ "uploads_allowed": input.allowed })))
}

macro_rules! h {
    ($name:ident, $inner:ident) => {
        fn $name(
            request: Request,
            _: PathParams,
            _: ConnectionInfo,
            ortus: Arc<Instance>,
        ) -> BoxFuture<'static, Response> {
            Box::pin(async move { answer(&request, $inner(&request, &ortus).await) })
        }
    };
    ($name:ident, $inner:ident, params) => {
        fn $name(
            request: Request,
            params: PathParams,
            _: ConnectionInfo,
            ortus: Arc<Instance>,
        ) -> BoxFuture<'static, Response> {
            Box::pin(async move { answer(&request, $inner(&request, &params, &ortus).await) })
        }
    };
}

/// Photos answer with the image itself, so a failure is the API's JSON
/// error and success the bytes.
fn image_answer(request: &Request, result: Result<Response, ApiError>) -> Response {
    match result {
        Ok(response) => response,
        Err(e) => e.response(request),
    }
}

fn h_by_id(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { image_answer(&request, by_id(&request, &params, &ortus).await) })
}

fn h_by_address(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { image_answer(&request, by_address(&request, &ortus).await) })
}

h!(h_mine, mine);
h!(h_upload, upload);
h!(h_remove_mine, remove_mine);
h!(h_remove, remove, params);
h!(h_set_uploads, set_uploads);

fn any_object() -> Value {
    json!({"type": "object"})
}

fn image() -> Value {
    json!({"type": "string", "format": "binary", "description": "The image (PNG, JPEG or WebP)."})
}

pub fn routes() -> Vec<Route> {
    let r = |method, path, id, summary, access, response, handler| Route {
        method,
        path,
        id,
        tag: "people",
        summary,
        access,
        response,
        handler,
    };
    vec![
        r(
            Method::Get,
            "/me/photo",
            "getMyPhoto",
            "Whether you have a photo (its source: own or directory) and may upload one.",
            Access::Signed,
            any_object as fn() -> Value,
            h_mine as crate::api::Handler,
        ),
        r(
            Method::Put,
            "/me/photo",
            "setMyPhoto",
            "Your photo: the image itself as the body (PNG, JPEG or WebP, at most 512 KB).",
            Access::Signed,
            any_object,
            h_upload,
        ),
        r(
            Method::Delete,
            "/me/photo",
            "removeMyPhoto",
            "Remove your photo (your initials show instead).",
            Access::Signed,
            any_object,
            h_remove_mine,
        ),
        r(
            Method::Get,
            "/people/:id/photo",
            "getPhoto",
            "A colleague's photo, by account; 204 No Content when they have none.",
            Access::Signed,
            image,
            h_by_id,
        ),
        r(
            Method::Get,
            "/people/photo",
            "getPhotoByAddress",
            "A colleague's photo, by address (?email=); 204 No Content when there is none.",
            Access::Signed,
            image,
            h_by_address,
        ),
        r(
            Method::Delete,
            "/admin/users/:id/photo",
            "removePersonPhoto",
            "Remove someone's photo.",
            Access::Admin,
            any_object,
            h_remove,
        ),
        r(
            Method::Put,
            "/admin/photos",
            "setPhotoUploads",
            "Whether people may upload their own photo: {allowed}. Photos from the directory stay.",
            Access::Admin,
            any_object,
            h_set_uploads,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::image_type;

    #[test]
    fn only_png_jpeg_and_webp_are_photos() {
        assert_eq!(Some("image/png"), image_type(b"\x89PNG\r\n\x1a\n...."));
        assert_eq!(Some("image/jpeg"), image_type(b"\xff\xd8\xff\xe0...."));
        assert_eq!(Some("image/webp"), image_type(b"RIFF\0\0\0\0WEBPVP8 "));
        assert_eq!(
            None,
            image_type(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>")
        );
        assert_eq!(None, image_type(b"GIF89a"));
        assert_eq!(None, image_type(b""));
    }
}
