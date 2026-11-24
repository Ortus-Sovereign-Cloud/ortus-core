//! Restore points: the whole installation as it was, taken on a schedule
//! and kept hourly for 48 hours, daily for 30 days and weekly for a year.
//!
//! They go to a store of their own, never the installation's: the
//! directory `ORTUS_BACKUP_DIR`, or an S3 bucket (`ORTUS_BACKUP_S3_ENDPOINT`,
//! `_BUCKET`, `_REGION`, `_ACCESS_KEY`, `_SECRET_KEY`, `_ROOT`); by default
//! `<data dir>/backups`. The layout:
//!
//! ```text
//! points/<name>/tables/<table>.jsonl   as in a backup (see crate::backup)
//! points/<name>/files.jsonl            {"key", "size", "sha256"} per file
//! points/<name>/manifest.json          written last: the point is whole
//! blobs/<sha256>                       each file's contents, once
//! ```
//!
//! A file's contents are stored once however many points name it; an
//! index of key to checksum spares reading unchanged files (keys of Ortus
//! files do not change contents), and the first point of each day reads
//! every file again. A point exports to a backup directory that `ortus
//! restore` takes (`ortus export-point <name> <dir>`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};

use crate::backup::{BackupError, FilesEntry, Manifest, FORMAT, FORMAT_VERSION};
use crate::files::{Files, S3Settings};
use crate::instance::Instance;
use crate::storage::{AsyncStore, Migration, Schema};

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "restore points: the file index and the points taken",
        sql: "CREATE TABLE core_backup_index (
        key TEXT PRIMARY KEY,
        size BIGINT NOT NULL,
        sha256 TEXT NOT NULL
    );
    CREATE TABLE core_points (
        name TEXT PRIMARY KEY,
        at BIGINT NOT NULL,
        tables BIGINT NOT NULL,
        files BIGINT NOT NULL,
        bytes BIGINT NOT NULL
    );
    CREATE TABLE core_restore_tests (
        at BIGINT PRIMARY KEY,
        point TEXT NOT NULL,
        ok BIGINT NOT NULL,
        duration_ms BIGINT NOT NULL,
        detail TEXT NOT NULL
    )",
    },
    Migration {
        version: 2,
        name: "restore points: one taker or pruner at a time",
        sql: "CREATE TABLE core_points_lease (
        id BIGINT PRIMARY KEY,
        until_ms BIGINT NOT NULL
    );
    INSERT INTO core_points_lease (id, until_ms) VALUES (1, 0)",
    },
];

pub const SCHEMA: Schema = Schema {
    app: "core.points",
    migrations: MIGRATIONS,
};

/// The job that takes points when they are due (every ten minutes it
/// looks).
pub const JOB: &str = "core.points";

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

/// Where restore points go.
pub fn destination(data_dir: &Path) -> Result<Files, String> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    if let Some(endpoint) = var("ORTUS_BACKUP_S3_ENDPOINT") {
        let need = |name: &str| var(name).ok_or_else(|| format!("{name} is not set"));
        return Files::s3(&S3Settings {
            endpoint,
            bucket: need("ORTUS_BACKUP_S3_BUCKET")?,
            region: var("ORTUS_BACKUP_S3_REGION").unwrap_or_else(|| "us-east-1".into()),
            access_key: need("ORTUS_BACKUP_S3_ACCESS_KEY")?,
            secret_key: need("ORTUS_BACKUP_S3_SECRET_KEY")?,
            root: var("ORTUS_BACKUP_S3_ROOT").unwrap_or_default(),
        })
        .map_err(|e| e.to_string());
    }
    let dir = var("ORTUS_BACKUP_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| data_dir.join("backups"));
    Files::local(&dir).map_err(|e| e.to_string())
}

/// Copies the file under `key` into `blob` a chunk at a time, if it still
/// has the checksum `sum`; a copy that turns out different is not kept.
async fn copy_blob(
    files: &Files,
    key: &str,
    dest: &Files,
    blob: &str,
    sum: &str,
) -> Result<bool, BackupError> {
    let mut out = dest.writer(blob).await?;
    let (mut hasher, mut at) = (Sha256::new(), 0u64);
    loop {
        let Some(part) = files.read_range(key, at, crate::backup::CHUNK).await? else {
            out.abort().await?;
            return Ok(false);
        };
        hasher.update(&part);
        out.write(&part).await?;
        at += part.len() as u64;
        if (part.len() as u64) < crate::backup::CHUNK {
            break;
        }
    }
    if hex::encode(hasher.finalize()) != sum {
        out.abort().await?;
        return Ok(false);
    }
    out.close().await?;
    Ok(true)
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[derive(Serialize, Deserialize)]
struct FileLine {
    key: String,
    size: u64,
    sha256: String,
}

/// A point as listed.
#[derive(Clone, Debug, Serialize)]
pub struct Point {
    pub name: String,
    pub at: i64,
    pub tables: i64,
    pub files: i64,
    pub bytes: i64,
}

/// A point's name from its time: "20261120-180000".
pub fn name_at(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// How long one taker or pruner may hold the backup store.
const LEASE: i64 = 6 * HOUR;

/// Claims the backup store for taking or pruning (one at a time, across
/// workers and the "take one now" button). False when someone has it.
pub async fn claim(instance: &Instance) -> Result<bool, BackupError> {
    let now = crate::program::now_ms();
    Ok(instance
        .store
        .execute(
            "UPDATE core_points_lease SET until_ms = $1 WHERE id = 1 AND until_ms <= $2",
            &[(now + LEASE).into(), now.into()],
        )
        .await?
        == 1)
}

pub async fn release(instance: &Instance) {
    let _ = instance
        .store
        .execute(
            "UPDATE core_points_lease SET until_ms = 0 WHERE id = 1",
            &[],
        )
        .await;
}

/// When a point was taken, from its name ("20261120-180000").
pub fn time_of(name: &str) -> Option<i64> {
    let (date, time) = name.split_once('-')?;
    if date.len() != 8 || time.len() != 6 {
        return None;
    }
    let num = |s: &str| s.parse::<i64>().ok();
    let (y, m, d) = (num(&date[..4])?, num(&date[4..6])?, num(&date[6..])?);
    let (hh, mm, ss) = (num(&time[..2])?, num(&time[2..4])?, num(&time[4..])?);
    // Days since 1970 from a civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + hh * 3600 + mm * 60 + ss) * 1000)
}

/// The backup store's own id (a file in it). The file index is only good
/// for the store it was made against: a new or emptied store, or another
/// one, starts the index again.
async fn same_store(instance: &Instance, dest: &Files) -> Result<bool, BackupError> {
    let id = match dest.get("destination.id").await? {
        Some(id) => String::from_utf8_lossy(&id).trim().to_string(),
        None => {
            let id = crate::auth::random_hex(16);
            dest.put("destination.id", id.as_bytes()).await?;
            id
        }
    };
    let s = instance.store.as_ref();
    let known = crate::auth::policy::setting(s, "backup_destination").await?;
    if known.as_deref() == Some(id.as_str()) {
        return Ok(true);
    }
    s.execute("DELETE FROM core_backup_index", &[]).await?;
    crate::auth::policy::set_setting(s, "backup_destination", &id).await?;
    Ok(false)
}

/// Takes a point now. `full` reads every file again (else the index
/// spares unchanged ones).
pub async fn take(
    instance: &Instance,
    dest: &Files,
    now: i64,
    full: bool,
) -> Result<Point, BackupError> {
    let store = instance.store.as_ref();
    let name = name_at(now);
    let base = format!("points/{name}");
    // Another store than the index knows: every file read again.
    let full = !same_store(instance, dest).await? || full;
    // The tables, from one consistent snapshot.
    let engine = store.engine();
    let mut tx = store.begin_read().await?;
    let names = crate::backup::table_names(&mut *tx, engine).await?;
    let mut entries = Vec::new();
    for table in &names {
        // A page at a time: a big table never sits in memory whole.
        let mut out = dest.writer(&format!("{base}/tables/{table}.jsonl")).await?;
        let mut dump = crate::backup::TableDump::start(&mut *tx, engine, table).await?;
        while let Some(page) = dump.next(&mut *tx).await? {
            out.write(page.as_bytes()).await?;
        }
        out.close().await?;
        entries.push(dump.entry());
    }
    let history = tx
        .query(
            "SELECT app, MAX(version) FROM ortus_migrations GROUP BY app",
            &[],
        )
        .await?;
    tx.commit().await?;
    let schemas: BTreeMap<String, i64> = history
        .iter()
        .filter_map(|r| Some((r.text(0)?.to_string(), r.int(1)?)))
        .collect();

    // The files: each one's contents stored once, by checksum.
    let known: BTreeMap<String, (u64, String)> = store
        .query("SELECT key, size, sha256 FROM core_backup_index", &[])
        .await?
        .iter()
        .filter_map(|r| {
            Some((
                r.text(0)?.to_string(),
                (r.int(1)? as u64, r.text(2)?.to_string()),
            ))
        })
        .collect();
    let mut listing = String::new();
    let (mut count, mut bytes) = (0u64, 0u64);
    let mut skipped = Vec::new();
    for key in instance.files.list("").await? {
        let (size, sum) = match known.get(&key).filter(|_| !full) {
            Some((size, sum)) => (*size, sum.clone()),
            None => {
                // Summed, then copied, a chunk at a time: a big file never
                // sits in memory whole, and one already kept is not copied.
                let Some((size, sum)) = crate::backup::file_sum(&instance.files, &key).await?
                else {
                    skipped.push(key);
                    continue;
                };
                let blob = format!("blobs/{sum}");
                if !dest.exists(&blob).await?
                    && !copy_blob(&instance.files, &key, dest, &blob, &sum).await?
                {
                    // Changed or gone between summing and copying.
                    skipped.push(key);
                    continue;
                }
                if known.get(&key).map(|(s, h)| (*s, h.as_str())) != Some((size, sum.as_str())) {
                    let mut t = store.begin().await?;
                    t.execute(
                        "DELETE FROM core_backup_index WHERE key = $1",
                        &[key.as_str().into()],
                    )
                    .await?;
                    t.execute(
                        "INSERT INTO core_backup_index (key, size, sha256) VALUES ($1, $2, $3)",
                        &[
                            key.as_str().into(),
                            (size as i64).into(),
                            sum.as_str().into(),
                        ],
                    )
                    .await?;
                    t.commit().await?;
                }
                (size, sum)
            }
        };
        listing.push_str(
            &serde_json::to_string(&FileLine {
                key,
                size,
                sha256: sum,
            })
            .map_err(|e| BackupError(e.to_string()))?,
        );
        listing.push('\n');
        count += 1;
        bytes += size;
    }
    dest.put(&format!("{base}/files.jsonl"), listing.as_bytes())
        .await?;
    let manifest = Manifest {
        format: FORMAT.into(),
        format_version: FORMAT_VERSION,
        ortus_version: crate::VERSION.into(),
        created_at_ms: now,
        engine: if engine == crate::storage::Engine::Sqlite {
            "sqlite"
        } else {
            "postgres"
        }
        .into(),
        schemas,
        tables: entries,
        files: FilesEntry {
            count,
            bytes,
            sha256: sha256(listing.as_bytes()),
            skipped,
        },
    };
    // The manifest last: a point without one was cut short. Before it, its
    // checksum sealed with this installation's key, so a point changed in
    // the backup store (which may be shared or less guarded) is refused.
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| BackupError(e.to_string()))?;
    dest.put(
        &format!("{base}/manifest.seal"),
        instance
            .keys
            .seal_text(&sealed(&name, text.as_bytes()))
            .as_bytes(),
    )
    .await?;
    dest.put(&format!("{base}/manifest.json"), text.as_bytes())
        .await?;
    let point = Point {
        name: name.clone(),
        at: now,
        tables: manifest.tables.len() as i64,
        files: count as i64,
        bytes: bytes as i64,
    };
    let mut t = store.begin().await?;
    t.execute(
        "DELETE FROM core_points WHERE name = $1",
        &[name.as_str().into()],
    )
    .await?;
    t.execute(
        "INSERT INTO core_points (name, at, tables, files, bytes) VALUES ($1, $2, $3, $4, $5)",
        &[
            name.as_str().into(),
            now.into(),
            point.tables.into(),
            point.files.into(),
            point.bytes.into(),
        ],
    )
    .await?;
    t.commit().await?;
    Ok(point)
}

/// The points to keep, of those taken at these times: the newest of each
/// hour for 48 hours, of each day for 30 days, of each week for a year,
/// and the newest of all.
pub fn keep(times: &[i64], now: i64) -> BTreeSet<i64> {
    let mut sorted: Vec<i64> = times.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let mut kept = BTreeSet::new();
    if let Some(newest) = sorted.first() {
        kept.insert(*newest);
    }
    for (span, bucket) in [(48 * HOUR, HOUR), (30 * DAY, DAY), (365 * DAY, 7 * DAY)] {
        let mut seen = BTreeSet::new();
        for t in &sorted {
            if now - t <= span && seen.insert(t.div_euclid(bucket)) {
                kept.insert(*t);
            }
        }
    }
    kept
}

/// Points that are whole (with a manifest), by name, with their times.
async fn listed(dest: &Files) -> Result<Vec<(String, i64)>, BackupError> {
    let mut out = Vec::new();
    for key in dest.list("points/").await? {
        let Some(name) = key
            .strip_prefix("points/")
            .and_then(|k| k.strip_suffix("/manifest.json"))
        else {
            continue;
        };
        let Some(text) = dest.get(&key).await? else {
            continue;
        };
        let Ok(m) = serde_json::from_slice::<Manifest>(&text) else {
            continue;
        };
        out.push((name.to_string(), m.created_at_ms));
    }
    Ok(out)
}

/// Removes the points no longer kept, and the file contents no kept point
/// names. Returns how many points went.
pub async fn prune(instance: &Instance, dest: &Files, now: i64) -> Result<usize, BackupError> {
    let points = listed(dest).await?;
    let times: Vec<i64> = points.iter().map(|(_, t)| *t).collect();
    let kept = keep(&times, now);
    let mut removed = 0;
    for (name, at) in &points {
        if kept.contains(at) {
            continue;
        }
        for key in dest.list(&format!("points/{name}/")).await? {
            dest.delete(&key).await?;
        }
        instance
            .store
            .execute(
                "DELETE FROM core_points WHERE name = $1",
                &[name.as_str().into()],
            )
            .await?;
        removed += 1;
    }
    // Leftovers of points cut short (no manifest, and older than a take
    // could last) go too.
    let all = dest.list("points/").await?;
    let with_manifest: BTreeSet<&str> = all
        .iter()
        .filter_map(|k| k.strip_prefix("points/")?.strip_suffix("/manifest.json"))
        .collect();
    for key in &all {
        let name = key
            .strip_prefix("points/")
            .and_then(|k| k.split('/').next())
            .unwrap_or_default();
        let old = time_of(name).is_some_and(|t| now - t > LEASE);
        if !with_manifest.contains(name) && old {
            dest.delete(key).await?;
        }
    }
    if removed > 0 {
        // Contents no point names any more.
        let mut named = BTreeSet::new();
        for (name, at) in &points {
            if !kept.contains(at) {
                continue;
            }
            if let Some(text) = dest.get(&format!("points/{name}/files.jsonl")).await? {
                for line in String::from_utf8_lossy(&text).lines() {
                    if let Ok(f) = serde_json::from_str::<FileLine>(line) {
                        named.insert(f.sha256);
                    }
                }
            }
        }
        for key in dest.list("blobs/").await? {
            let sum = key.trim_start_matches("blobs/");
            if !named.contains(sum) {
                dest.delete(&key).await?;
                instance
                    .store
                    .execute(
                        "DELETE FROM core_backup_index WHERE sha256 = $1",
                        &[sum.into()],
                    )
                    .await?;
            }
        }
    }
    Ok(removed)
}

/// What a point's seal covers: its name (an older manifest cannot pass
/// for a newer point) and its manifest's checksum.
fn sealed(name: &str, manifest: &[u8]) -> String {
    format!("{name}:{}", sha256(manifest))
}

/// Whether point `name` is as this installation took it: its manifest's
/// checksum, sealed with this installation's keys, matches.
pub async fn verify(
    dest: &Files,
    name: &str,
    keys: &crate::secrets::Keyring,
) -> Result<(), BackupError> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return Err(BackupError(format!("'{name}' is not a restore point")));
    }
    let base = format!("points/{name}");
    let refused = || {
        BackupError(format!(
            "restore point {name} is not as this installation took it"
        ))
    };
    let text = dest
        .get(&format!("{base}/manifest.json"))
        .await?
        .ok_or_else(refused)?;
    let seal = dest
        .get(&format!("{base}/manifest.seal"))
        .await?
        .ok_or_else(refused)?;
    let opened = keys
        .open_text(&String::from_utf8_lossy(&seal))
        .map_err(|_| refused())?;
    if opened != sealed(name, &text) {
        return Err(refused());
    }
    Ok(())
}

/// Writes a point out as a backup directory (`ortus restore` takes it).
pub async fn export(dest: &Files, name: &str, dir: &Path) -> Result<Manifest, BackupError> {
    if !name.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return Err(BackupError(format!("'{name}' is not a restore point")));
    }
    let base = format!("points/{name}");
    let text = dest
        .get(&format!("{base}/manifest.json"))
        .await?
        .ok_or_else(|| BackupError(format!("there is no whole restore point {name}")))?;
    let manifest: Manifest =
        serde_json::from_slice(&text).map_err(|e| BackupError(e.to_string()))?;
    let io = |p: &Path| {
        let p = p.to_path_buf();
        move |e: std::io::Error| BackupError(format!("{}: {e}", p.display()))
    };
    std::fs::create_dir_all(dir.join("tables")).map_err(io(dir))?;
    for t in &manifest.tables {
        crate::backup::plain_name(&t.name)?;
        let rows = dest
            .get(&format!("{base}/tables/{}.jsonl", t.name))
            .await?
            .ok_or_else(|| BackupError(format!("the point lacks table {}", t.name)))?;
        let path = dir.join("tables").join(format!("{}.jsonl", t.name));
        std::fs::write(&path, rows).map_err(io(&path))?;
    }
    let listing = dest
        .get(&format!("{base}/files.jsonl"))
        .await?
        .unwrap_or_default();
    std::fs::write(dir.join("files.jsonl"), &listing).map_err(io(dir))?;
    for line in String::from_utf8_lossy(&listing).lines() {
        let f: FileLine = serde_json::from_str(line).map_err(|e| BackupError(e.to_string()))?;
        // Keys and checksums are checked before they become paths.
        crate::files::check(&f.key)?;
        if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(BackupError(format!("{}: not a checksum", f.sha256)));
        }
        let contents = dest
            .get(&format!("blobs/{}", f.sha256))
            .await?
            .ok_or_else(|| BackupError(format!("the contents of {} are missing", f.key)))?;
        let path = dir.join("files").join(&f.key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::write(&path, contents).map_err(io(&path))?;
    }
    let mut text =
        serde_json::to_string_pretty(&manifest).map_err(|e| BackupError(e.to_string()))?;
    text.push('\n');
    std::fs::write(dir.join("manifest.json"), text).map_err(io(dir))?;
    Ok(manifest)
}

/// The scheduled run: a point when the last is an hour old (reading every
/// file on the first of the day), then pruning.
pub async fn run(instance: &Instance) -> Result<(), String> {
    let now = crate::program::now_ms();
    let s = instance.store.as_ref();
    let last = s
        .query("SELECT MAX(at) FROM core_points", &[])
        .await
        .map_err(|e| e.to_string())?
        .first()
        .and_then(|r| r.int(0));
    if last.is_some_and(|t| now - t < HOUR - 5 * 60_000) {
        return Ok(());
    }
    // One at a time: another worker (or the button) may be at it.
    if !claim(instance).await.map_err(|e| e.to_string())? {
        return Ok(());
    }
    let dest = destination(&instance.config.data_dir)?;
    let full = last.is_none_or(|t| t.div_euclid(DAY) != now.div_euclid(DAY));
    let done = async {
        let point = take(instance, &dest, now, full).await?;
        let removed = prune(instance, &dest, now).await?;
        Ok::<_, BackupError>((point, removed))
    }
    .await;
    release(instance).await;
    let (point, removed) = done.map_err(|e| e.to_string())?;
    crate::audit::record(s, &crate::audit::Event::new("backup.point_taken", crate::audit::Outcome::Done).with(json!({"point": point.name, "files": point.files, "bytes": point.bytes, "pruned": removed})), now)
        .await
        .map_err(|e| e.to_string())?;
    // Once a month, the new point is restored whole as a test.
    let tested = s
        .query("SELECT MAX(at) FROM core_restore_tests", &[])
        .await
        .map_err(|e| e.to_string())?
        .first()
        .and_then(|r| r.int(0));
    if tested.is_none_or(|t| now - t >= TEST_EVERY) {
        // Its own job, so the worker's other jobs need not wait for it.
        let id = format!("restore-test-{}", name_at(now).get(..6).unwrap_or_default());
        crate::jobs::enqueue(
            s,
            &crate::jobs::NewJob {
                id: &id,
                kind: TEST_JOB,
                payload: &point.name,
                run_at: now,
                max_attempts: 1,
            },
            now,
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The outside job that runs it.
pub fn job(instance: Arc<Instance>) -> crate::app::OutsideJobKind {
    crate::app::OutsideJobKind {
        kind: JOB.into(),
        handler: Box::new(move |_| {
            let instance = Arc::clone(&instance);
            Box::pin(async move { run(&instance).await })
        }),
    }
}

/// The job that tests a restore on demand: its payload is the point (the
/// newest when empty).
pub fn test_job(instance: Arc<Instance>) -> crate::app::OutsideJobKind {
    crate::app::OutsideJobKind {
        kind: TEST_JOB.into(),
        handler: Box::new(move |job| {
            let instance = Arc::clone(&instance);
            let wanted = job.payload.clone();
            Box::pin(async move {
                let dest = destination(&instance.config.data_dir)?;
                let name = if wanted.is_empty() {
                    instance
                        .store
                        .query("SELECT name FROM core_points ORDER BY at DESC LIMIT 1", &[])
                        .await
                        .map_err(|e| e.to_string())?
                        .first()
                        .and_then(|r| r.text(0).map(str::to_string))
                        .ok_or("there is no restore point to test")?
                } else {
                    wanted
                };
                // Not while a point is being taken or pruned (its files
                // could go under the test): wait up to half an hour.
                let mut held = false;
                for _ in 0..180 {
                    if claim(&instance).await.map_err(|e| e.to_string())? {
                        held = true;
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
                if !held {
                    return Err("the backup store stayed busy".into());
                }
                test_restore(&instance, &dest, &name).await;
                release(&instance).await;
                Ok(())
            })
        }),
    }
}

/// The points, newest first, and the last restore test.
pub async fn summary(instance: &Instance) -> Result<Json, String> {
    let s = instance.store.as_ref();
    let points: Vec<Json> = s
        .query("SELECT name, at, tables, files, bytes FROM core_points ORDER BY at DESC LIMIT 200", &[])
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .map(|r| json!({"name": r.text(0), "at": r.int(1), "tables": r.int(2), "files": r.int(3), "bytes": r.int(4)}))
        .collect();
    let test = s
        .query("SELECT at, point, ok, duration_ms, detail FROM core_restore_tests ORDER BY at DESC LIMIT 1", &[])
        .await
        .map_err(|e| e.to_string())?
        .first()
        .map(|r| json!({"at": r.int(0), "point": r.text(1), "ok": r.int(2).unwrap_or(0) != 0, "durationMs": r.int(3), "detail": r.text(4)}));
    let self_restore = self_restore(instance).await;
    let restorable: Vec<Json> = instance
        .apps
        .iter()
        .flat_map(|a| {
            a.restorable()
                .iter()
                .map(move |w| json!({"app": a.info().id, "what": w}))
        })
        .collect();
    Ok(
        json!({"points": points, "lastTest": test, "selfRestore": self_restore, "restorable": restorable}),
    )
}

async fn get_points(
    request: &rust_web_server::prelude::Request,
    instance: &Arc<Instance>,
) -> Result<rust_web_server::prelude::Response, crate::api::ApiError> {
    crate::auth::current(request, instance)
        .await
        .ok_or_else(crate::api::ApiError::sign_in)?;
    Ok(crate::api::ok(
        &summary(instance)
            .await
            .map_err(|_| crate::api::ApiError::internal())?,
    ))
}

async fn take_now(
    request: &rust_web_server::prelude::Request,
    instance: &Arc<Instance>,
) -> Result<rust_web_server::prelude::Response, crate::api::ApiError> {
    let me = crate::auth::current(request, instance)
        .await
        .ok_or_else(crate::api::ApiError::sign_in)?
        .user;
    let dest =
        destination(&instance.config.data_dir).map_err(|_| crate::api::ApiError::internal())?;
    let now = crate::program::now_ms();
    if !claim(instance)
        .await
        .map_err(|_| crate::api::ApiError::internal())?
    {
        return Err(crate::api::ApiError::conflict(
            "A restore point is being taken; try again in a while.",
        ));
    }
    let taken = take(instance, &dest, now, false).await;
    release(instance).await;
    let point = taken.map_err(|e| crate::api::ApiError::bad_request(e.to_string()))?;
    crate::auth::audit(
        instance,
        crate::audit::Event::new("backup.point_taken", crate::audit::Outcome::Done)
            .by(&me.id)
            .with(json!({"point": point.name})),
    )
    .await;
    Ok(crate::api::ok(
        &summary(instance)
            .await
            .map_err(|_| crate::api::ApiError::internal())?,
    ))
}

fn h_points(
    request: rust_web_server::prelude::Request,
    _: rust_web_server::prelude::PathParams,
    _: rust_web_server::prelude::ConnectionInfo,
    instance: Arc<Instance>,
) -> crate::storage::BoxFuture<'static, rust_web_server::prelude::Response> {
    Box::pin(async move { crate::api::answer(&request, get_points(&request, &instance).await) })
}

fn h_take(
    request: rust_web_server::prelude::Request,
    _: rust_web_server::prelude::PathParams,
    _: rust_web_server::prelude::ConnectionInfo,
    instance: Arc<Instance>,
) -> crate::storage::BoxFuture<'static, rust_web_server::prelude::Response> {
    Box::pin(async move { crate::api::answer(&request, take_now(&request, &instance).await) })
}

pub fn routes() -> Vec<crate::api::Route> {
    let object = || json!({"type": "object"});
    vec![
        crate::api::Route { method: crate::api::Method::Get, path: "/admin/backup", id: "backupPoints", tag: "admin", summary: "Restore points (hourly for 48 hours, daily for 30 days, weekly for a year) and the last restore test.", access: crate::auth::Access::Admin, response: object, handler: h_points },
        crate::api::Route { method: crate::api::Method::Post, path: "/admin/backup/points", id: "backupTakePoint", tag: "admin", summary: "Takes a restore point now.", access: crate::auth::Access::Owner, response: object, handler: h_take },
        crate::api::Route { method: crate::api::Method::Post, path: "/admin/backup/tests", id: "backupTestRestore", tag: "admin", summary: "Tests a restore now: the newest point restored whole into a scratch installation and checked (rows, files, keys), timed and recorded. Runs in the background; GET /admin/backup shows the result.", access: crate::auth::Access::Owner, response: object, handler: h_test },
        crate::api::Route { method: crate::api::Method::Patch, path: "/admin/backup", id: "backupSettings", tag: "admin", summary: "{selfRestore}: whether people bring back their own things themselves.", access: crate::auth::Access::Owner, response: object, handler: h_settings },
        crate::api::Route { method: crate::api::Method::Get, path: "/backup/choices", id: "backupChoices", tag: "backup", summary: "?point=&app=&what=&user=(address): a person's things of that kind as they were at the point (an administrator, or the person when self-restore is on).", access: crate::auth::Access::Signed, response: object, handler: h_choices },
        crate::api::Route { method: crate::api::Method::Post, path: "/backup/restore", id: "backupRestore", tag: "backup", summary: "{point, app, what, user (address), id}: brings one back, as it was, into a new place beside the person's own.", access: crate::auth::Access::Signed, response: object, handler: h_restore },
    ]
}

/// A restore point opened for reading: its tables' rows and its files.
pub struct Snapshot {
    dest: Files,
    base: String,
    pub manifest: Manifest,
    /// File key to checksum, read the first time a file is wanted.
    files: tokio::sync::OnceCell<BTreeMap<String, String>>,
}

impl Snapshot {
    /// Opens the whole point `name` in `dest`.
    pub async fn open(dest: Files, name: &str) -> Result<Snapshot, String> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return Err(format!("'{name}' is not a restore point"));
        }
        let base = format!("points/{name}");
        let text = dest
            .get(&format!("{base}/manifest.json"))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("there is no whole restore point {name}"))?;
        let manifest: Manifest = serde_json::from_slice(&text).map_err(|e| e.to_string())?;
        Ok(Snapshot {
            dest,
            base,
            manifest,
            files: tokio::sync::OnceCell::new(),
        })
    }

    /// When the point was taken.
    pub fn at(&self) -> i64 {
        self.manifest.created_at_ms
    }

    /// The rows of `table` (column name to value; a blob is {"hex"}) that
    /// `keep` takes. None when the point has no such table.
    pub async fn rows(
        &self,
        table: &str,
        keep: impl Fn(&serde_json::Map<String, Json>) -> bool,
    ) -> Result<Vec<serde_json::Map<String, Json>>, String> {
        let Some(entry) = self.manifest.tables.iter().find(|t| t.name == table) else {
            return Ok(Vec::new());
        };
        let text = self
            .dest
            .get(&format!("{}/tables/{table}.jsonl", self.base))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("the point lacks table {table}"))?;
        if sha256(&text) != entry.sha256 {
            return Err(format!("table {table} in the point is damaged"));
        }
        let mut out = Vec::new();
        for line in String::from_utf8_lossy(&text).lines() {
            let values: Vec<Json> = serde_json::from_str(line).map_err(|e| e.to_string())?;
            let row: serde_json::Map<String, Json> =
                entry.columns.iter().cloned().zip(values).collect();
            if keep(&row) {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// A file's contents as they were (checked against its checksum).
    pub async fn file(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let index = self
            .files
            .get_or_try_init(|| async {
                let listing = self
                    .dest
                    .get(&format!("{}/files.jsonl", self.base))
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or("the point lacks its list of files")?;
                // The list as the (sealed) manifest names it.
                if sha256(&listing) != self.manifest.files.sha256 {
                    return Err("the point's list of files is damaged".to_string());
                }
                let mut map = BTreeMap::new();
                for line in String::from_utf8_lossy(&listing).lines() {
                    let f = serde_json::from_str::<FileLine>(line).map_err(|e| e.to_string())?;
                    if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err(format!("{}: not a checksum", f.sha256));
                    }
                    map.insert(f.key, f.sha256);
                }
                Ok::<_, String>(map)
            })
            .await?;
        let Some(sum) = index.get(key) else {
            return Ok(None);
        };
        let contents = self
            .dest
            .get(&format!("blobs/{sum}"))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("the contents of {key} are missing"))?;
        if &sha256(&contents) != sum {
            return Err(format!("the contents of {key} are damaged"));
        }
        Ok(Some(contents))
    }
}

/// Text of a row's column.
pub fn text<'a>(row: &'a serde_json::Map<String, Json>, column: &str) -> &'a str {
    row.get(column).and_then(Json::as_str).unwrap_or_default()
}

/// Number of a row's column.
pub fn int(row: &serde_json::Map<String, Json>, column: &str) -> Option<i64> {
    row.get(column).and_then(Json::as_i64)
}

async fn self_restore(instance: &Instance) -> bool {
    crate::auth::policy::setting(instance.store.as_ref(), "self_restore")
        .await
        .ok()
        .flatten()
        .is_some_and(|v| v == "on")
}

#[derive(Deserialize)]
struct BackupSettings {
    #[serde(rename = "selfRestore")]
    self_restore: Option<bool>,
}

async fn settings(
    request: &rust_web_server::prelude::Request,
    instance: &Arc<Instance>,
) -> Result<rust_web_server::prelude::Response, crate::api::ApiError> {
    let me = crate::auth::current(request, instance)
        .await
        .ok_or_else(crate::api::ApiError::sign_in)?
        .user;
    let input: BackupSettings = crate::api::body(request)?;
    if let Some(on) = input.self_restore {
        crate::auth::policy::set_setting(
            instance.store.as_ref(),
            "self_restore",
            if on { "on" } else { "off" },
        )
        .await?;
        crate::auth::audit(
            instance,
            crate::audit::Event::new("backup.self_restore", crate::audit::Outcome::Done)
                .by(&me.id)
                .with(json!({"on": on})),
        )
        .await;
    }
    Ok(crate::api::ok(
        &summary(instance)
            .await
            .map_err(|_| crate::api::ApiError::internal())?,
    ))
}

/// The person a restore is for, if the one asking may restore for them:
/// an administrator for anyone, a person for themselves when self-restore
/// is on.
async fn for_whom(
    instance: &Arc<Instance>,
    me: &crate::auth::accounts::User,
    email: &str,
) -> Result<crate::auth::accounts::User, crate::api::ApiError> {
    let whom = crate::auth::accounts::by_email(
        instance.store.as_ref(),
        &email.trim().to_ascii_lowercase(),
    )
    .await?
    .ok_or_else(|| crate::api::ApiError::not_found("There is no such person."))?;
    let allowed = me.role >= crate::auth::accounts::Role::Admin
        || (whom.id == me.id && self_restore(instance).await);
    if allowed {
        Ok(whom)
    } else {
        Err(crate::api::ApiError::forbidden())
    }
}

fn restorer(
    instance: &Instance,
    app: &str,
    what: &str,
) -> Result<Arc<dyn crate::app::App>, crate::api::ApiError> {
    instance
        .apps
        .iter()
        .find(|a| a.info().id == app && a.restorable().contains(&what))
        .cloned()
        .ok_or_else(|| crate::api::ApiError::bad_request("Nothing of that kind is restored."))
}

async fn opened(instance: &Instance, name: &str) -> Result<Snapshot, crate::api::ApiError> {
    let dest =
        destination(&instance.config.data_dir).map_err(|_| crate::api::ApiError::internal())?;
    verify(&dest, name, &instance.keys)
        .await
        .map_err(|e| crate::api::ApiError::bad_request(e.to_string()))?;
    Snapshot::open(dest, name)
        .await
        .map_err(crate::api::ApiError::bad_request)
}

async fn choices(
    request: &rust_web_server::prelude::Request,
    instance: &Arc<Instance>,
) -> Result<rust_web_server::prelude::Response, crate::api::ApiError> {
    let me = crate::auth::current(request, instance)
        .await
        .ok_or_else(crate::api::ApiError::sign_in)?
        .user;
    let uri = &request.request_uri;
    let q = |n: &str| crate::audit::query_param(uri, n).unwrap_or_default();
    let whom = for_whom(instance, &me, &q("user")).await?;
    let app = restorer(instance, &q("app"), &q("what"))?;
    let point = opened(instance, &q("point")).await?;
    let list = app
        .restore_choices(&point, &whom.id, &q("what"))
        .await
        .map_err(crate::api::ApiError::bad_request)?;
    let items: Vec<Json> = list
        .iter()
        .map(|(id, name)| json!({"id": id, "name": name}))
        .collect();
    Ok(crate::api::ok(&json!({"at": point.at(), "choices": items})))
}

#[derive(Deserialize)]
struct Restore {
    point: String,
    app: String,
    what: String,
    user: String,
    id: String,
}

async fn restore_one(
    request: &rust_web_server::prelude::Request,
    instance: &Arc<Instance>,
) -> Result<rust_web_server::prelude::Response, crate::api::ApiError> {
    let me = crate::auth::current(request, instance)
        .await
        .ok_or_else(crate::api::ApiError::sign_in)?
        .user;
    let input: Restore = crate::api::body(request)?;
    let whom = for_whom(instance, &me, &input.user).await?;
    let app = restorer(instance, &input.app, &input.what)?;
    let point = opened(instance, &input.point).await?;
    let names: BTreeMap<String, String> = app
        .restore_choices(&point, &whom.id, &input.what)
        .await
        .map_err(crate::api::ApiError::bad_request)?
        .into_iter()
        .collect();
    let Some(name) = names.get(&input.id) else {
        return Err(crate::api::ApiError::not_found(
            "It was not there at that time.",
        ));
    };
    // "Agreements (as of 2026-11-17 18:00)": beside the person's own.
    let n = name_at(point.at());
    let into = format!(
        "{name} ({}-{}-{} {}:{})",
        &n[..4],
        &n[4..6],
        &n[6..8],
        &n[9..11],
        &n[11..13]
    );
    let path = app
        .restore(instance, &point, &whom.id, &input.what, &input.id, &into)
        .await
        .map_err(crate::api::ApiError::bad_request)?;
    crate::auth::audit(
        instance,
        crate::audit::Event::new("backup.restored", crate::audit::Outcome::Done)
            .by(&me.id)
            .on(&whom.id)
            .with(json!({"point": input.point, "app": input.app, "what": input.what, "id": input.id, "into": into})),
    )
    .await;
    Ok(crate::api::ok(&json!({"name": into, "path": path})))
}

fn h_settings(
    request: rust_web_server::prelude::Request,
    _: rust_web_server::prelude::PathParams,
    _: rust_web_server::prelude::ConnectionInfo,
    instance: Arc<Instance>,
) -> crate::storage::BoxFuture<'static, rust_web_server::prelude::Response> {
    Box::pin(async move { crate::api::answer(&request, settings(&request, &instance).await) })
}

fn h_choices(
    request: rust_web_server::prelude::Request,
    _: rust_web_server::prelude::PathParams,
    _: rust_web_server::prelude::ConnectionInfo,
    instance: Arc<Instance>,
) -> crate::storage::BoxFuture<'static, rust_web_server::prelude::Response> {
    Box::pin(async move { crate::api::answer(&request, choices(&request, &instance).await) })
}

fn h_restore(
    request: rust_web_server::prelude::Request,
    _: rust_web_server::prelude::PathParams,
    _: rust_web_server::prelude::ConnectionInfo,
    instance: Arc<Instance>,
) -> crate::storage::BoxFuture<'static, rust_web_server::prelude::Response> {
    Box::pin(async move { crate::api::answer(&request, restore_one(&request, &instance).await) })
}

/// The job that tests a restore (on demand, and monthly after a point).
pub const TEST_JOB: &str = "core.restore_test";
/// How often a restore is tested on its own.
const TEST_EVERY: i64 = 30 * DAY;

/// What a restore test found.
#[derive(Debug, Serialize)]
pub struct Tested {
    pub point: String,
    pub ok: bool,
    pub duration_ms: i64,
    pub detail: String,
}

/// Restores point `name` whole into a scratch installation beside the data
/// (its own SQLite database and file directory) and checks it: every
/// table's rows, every file, and that this installation's keys open what
/// it sealed. Timed and recorded; the scratch installation goes after.
pub async fn test_restore(instance: &Instance, dest: &Files, name: &str) -> Tested {
    let started = std::time::Instant::now();
    // Scratch installations a crash left behind.
    if let Ok(entries) = std::fs::read_dir(&instance.config.data_dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("restore-test-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let scratch = instance
        .config
        .data_dir
        .join(format!("restore-test-{}", crate::auth::random_hex(6)));
    let outcome = test_into(instance, dest, name, &scratch).await;
    let _ = std::fs::remove_dir_all(&scratch);
    let duration_ms = started.elapsed().as_millis() as i64;
    let (ok, detail) = match outcome {
        Ok(detail) => (true, detail),
        Err(e) => (false, e),
    };
    let now = crate::program::now_ms();
    let s = instance.store.as_ref();
    let _ = s
        .execute(
            "INSERT INTO core_restore_tests (at, point, ok, duration_ms, detail) VALUES ($1, $2, $3, $4, $5)",
            &[now.into(), name.into(), i64::from(ok).into(), duration_ms.into(), detail.as_str().into()],
        )
        .await;
    let _ = crate::audit::record(
        s,
        &crate::audit::Event::new(
            "backup.restore_tested",
            if ok {
                crate::audit::Outcome::Done
            } else {
                crate::audit::Outcome::Failed
            },
        )
        .with(json!({"point": name, "durationMs": duration_ms, "detail": detail})),
        now,
    )
    .await;
    Tested {
        point: name.to_string(),
        ok,
        duration_ms,
        detail,
    }
}

async fn test_into(
    instance: &Instance,
    dest: &Files,
    name: &str,
    scratch: &Path,
) -> Result<String, String> {
    let exported = scratch.join("backup");
    verify(dest, name, &instance.keys)
        .await
        .map_err(|e| e.to_string())?;
    export(dest, name, &exported)
        .await
        .map_err(|e| e.to_string())?;
    let schemas = crate::instance::schemas(&instance.apps);
    let checked = crate::backup::check(&exported, &schemas).map_err(|e| e.to_string())?;
    let target =
        crate::storage::SqliteStore::open(&scratch.join("ortus.db")).map_err(|e| e.to_string())?;
    let files = Files::local(&scratch.join("files")).map_err(|e| e.to_string())?;
    crate::backup::restore(
        &checked,
        &schemas,
        &target,
        &files,
        crate::program::now_ms(),
    )
    .await
    .map_err(|e| e.to_string())?;
    // Every table, row for row.
    for table in &checked.manifest.tables {
        let n = target
            .query(&format!("SELECT COUNT(*) FROM {}", table.name), &[])
            .await
            .map_err(|e| e.to_string())?
            .first()
            .and_then(|r| r.int(0))
            .unwrap_or(-1);
        if n != table.rows as i64 {
            return Err(format!(
                "table {}: {} rows restored of {}",
                table.name, n, table.rows
            ));
        }
    }
    // Every file.
    let restored = files.list("").await.map_err(|e| e.to_string())?.len() as u64;
    if restored != checked.manifest.files.count {
        return Err(format!(
            "{} files restored of {}",
            restored, checked.manifest.files.count
        ));
    }
    // This installation's keys open what it sealed.
    crate::secrets::check(&target, &instance.keys).await?;
    Ok(format!(
        "{} tables, {} files, keys open",
        checked.manifest.tables.len(),
        checked.manifest.files.count
    ))
}

async fn test_now(
    request: &rust_web_server::prelude::Request,
    instance: &Arc<Instance>,
) -> Result<rust_web_server::prelude::Response, crate::api::ApiError> {
    let me = crate::auth::current(request, instance)
        .await
        .ok_or_else(crate::api::ApiError::sign_in)?
        .user;
    let now = crate::program::now_ms();
    // One at a time: the job's id is the minute it was asked for.
    let id = format!("restore-test-{}", now / 60_000);
    crate::jobs::enqueue(
        instance.store.as_ref(),
        &crate::jobs::NewJob {
            id: &id,
            kind: TEST_JOB,
            payload: "",
            run_at: now,
            max_attempts: 1,
        },
        now,
    )
    .await?;
    crate::auth::audit(
        instance,
        crate::audit::Event::new("backup.restore_test_asked", crate::audit::Outcome::Done)
            .by(&me.id),
    )
    .await;
    Ok(crate::api::ok(&json!({"started": true})))
}

fn h_test(
    request: rust_web_server::prelude::Request,
    _: rust_web_server::prelude::PathParams,
    _: rust_web_server::prelude::ConnectionInfo,
    instance: Arc<Instance>,
) -> crate::storage::BoxFuture<'static, rust_web_server::prelude::Response> {
    Box::pin(async move { crate::api::answer(&request, test_now(&request, &instance).await) })
}
