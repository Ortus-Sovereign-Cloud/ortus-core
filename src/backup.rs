//! Backup and restore of a whole installation: its database and its files.
//!
//! A backup is a directory in a format that does not depend on the engine,
//! so a backup of the one-machine program (SQLite and a local directory)
//! restores into the Kubernetes chart (PostgreSQL and S3) and back:
//!
//! ```text
//! manifest.json          what is in the backup, with a checksum of each part
//! tables/<table>.jsonl   one JSON array per row, in column order
//! files.jsonl            one {"key", "size", "sha256"} per file
//! files/<key>            the contents of each file
//! ```
//!
//! The database is read in one read-only transaction, so the tables are a
//! consistent snapshot even while the program runs. Files are copied after
//! the tables; a file added in between is in the backup but no row points
//! at it yet, which does no harm.
//!
//! Restore goes into an empty database and an empty file store only. It
//! checks every checksum before it writes anything, creates each schema at
//! the version the backup was taken with, loads the rows in one
//! transaction and then copies the files. The program upgrades the schemas
//! as usual when it next starts. A backup taken by a newer program, or one
//! with a part this program does not have, is refused.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use crate::files::{self, Files};
use crate::storage::{migrate, AsyncStore, AsyncTx, Engine, Schema, StoreError, Value};

/// Name of the format, in every manifest.
pub const FORMAT: &str = "ortus-backup";
/// Version of the format this program writes and reads.
pub const FORMAT_VERSION: u32 = 1;

/// The table that records which migrations ran. It is in every backup.
const HISTORY: &str = "ortus_migrations";

/// A backup or restore that did not happen, and why.
#[derive(Debug)]
pub struct BackupError(pub String);

impl fmt::Display for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackupError {}

impl From<StoreError> for BackupError {
    fn from(e: StoreError) -> Self {
        BackupError(e.to_string())
    }
}

impl From<files::FilesError> for BackupError {
    fn from(e: files::FilesError) -> Self {
        BackupError(e.to_string())
    }
}

fn io(what: &Path) -> impl Fn(std::io::Error) -> BackupError + '_ {
    move |e| BackupError(format!("{}: {e}", what.display()))
}

/// What a backup holds; written as `manifest.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub format_version: u32,
    /// Version of the program that took the backup.
    pub ortus_version: String,
    pub created_at_ms: i64,
    /// Engine the backup was taken from: `sqlite` or `postgres`.
    pub engine: String,
    /// Schema version of each part, by its name in the migration history.
    pub schemas: BTreeMap<String, i64>,
    pub tables: Vec<TableEntry>,
    pub files: FilesEntry,
}

/// One table in a backup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableEntry {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: u64,
    /// SHA-256 of `tables/<name>.jsonl`.
    pub sha256: String,
}

/// The files in a backup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesEntry {
    pub count: u64,
    pub bytes: u64,
    /// SHA-256 of `files.jsonl`.
    pub sha256: String,
}

#[derive(Serialize, Deserialize)]
struct FileLine {
    key: String,
    size: u64,
    sha256: String,
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Table and column names come from the database or from a backup; only
/// plain lowercase names are ever put into SQL.
fn plain_name(name: &str) -> Result<&str, BackupError> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit());
    if ok {
        Ok(name)
    } else {
        Err(BackupError(format!(
            "'{name}' is not a table or column name Ortus uses"
        )))
    }
}

fn engine_name(engine: Engine) -> &'static str {
    match engine {
        Engine::Sqlite => "sqlite",
        Engine::Postgres => "postgres",
    }
}

async fn tables(tx: &mut dyn AsyncTx, engine: Engine) -> Result<Vec<String>, BackupError> {
    let sql = match engine {
        Engine::Sqlite => {
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        }
        Engine::Postgres => {
            "SELECT table_name::text FROM information_schema.tables WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' ORDER BY table_name"
        }
    };
    Ok(tx
        .query(sql, &[])
        .await?
        .iter()
        .filter_map(|row| row.text(0).map(str::to_string))
        .collect())
}

async fn columns(
    tx: &mut dyn AsyncTx,
    engine: Engine,
    table: &str,
) -> Result<Vec<String>, BackupError> {
    let sql = match engine {
        Engine::Sqlite => "SELECT name FROM pragma_table_info($1) ORDER BY cid",
        Engine::Postgres => {
            "SELECT column_name::text FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = $1 ORDER BY ordinal_position"
        }
    };
    Ok(tx
        .query(sql, &[table.into()])
        .await?
        .iter()
        .filter_map(|row| row.text(0).map(str::to_string))
        .collect())
}

/// The tables each table refers to with a foreign key.
async fn parents(
    tx: &mut dyn AsyncTx,
    engine: Engine,
    table: &str,
) -> Result<BTreeSet<String>, BackupError> {
    let sql = match engine {
        Engine::Sqlite => "SELECT DISTINCT \"table\" FROM pragma_foreign_key_list($1)",
        Engine::Postgres => {
            "SELECT DISTINCT parent.relname::text FROM pg_constraint c
             JOIN pg_class child ON c.conrelid = child.oid
             JOIN pg_class parent ON c.confrelid = parent.oid
             JOIN pg_namespace n ON child.relnamespace = n.oid
             WHERE c.contype = 'f' AND n.nspname = current_schema() AND child.relname = $1"
        }
    };
    Ok(tx
        .query(sql, &[table.into()])
        .await?
        .iter()
        .filter_map(|row| row.text(0).map(str::to_string))
        .filter(|parent| parent != table)
        .collect())
}

/// Tables in an order where every table comes after the tables it refers
/// to, so rows can be loaded one table at a time.
async fn load_order(
    tx: &mut dyn AsyncTx,
    engine: Engine,
    names: &[String],
) -> Result<Vec<String>, BackupError> {
    let mut pending = BTreeMap::new();
    for name in names {
        pending.insert(name.clone(), parents(tx, engine, name).await?);
    }
    let mut order = Vec::with_capacity(names.len());
    while !pending.is_empty() {
        let ready: Vec<String> = pending
            .iter()
            .filter(|(_, needs)| needs.iter().all(|n| !pending.contains_key(n)))
            .map(|(name, _)| name.clone())
            .collect();
        if ready.is_empty() {
            return Err(BackupError(format!(
                "the foreign keys between {} form a cycle",
                pending.keys().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        for name in ready {
            pending.remove(&name);
            order.push(name);
        }
    }
    Ok(order)
}

fn to_json(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Int(v) => Json::from(*v),
        Value::Text(v) => Json::from(v.as_str()),
        Value::Blob(v) => serde_json::json!({ "hex": hex::encode(v) }),
    }
}

fn from_json(value: &Json) -> Result<Value, BackupError> {
    let bad = || BackupError(format!("a value in the backup is not valid: {value}"));
    Ok(match value {
        Json::Null => Value::Null,
        Json::Number(n) => Value::Int(n.as_i64().ok_or_else(bad)?),
        Json::String(s) => Value::Text(s.clone()),
        Json::Object(o) => match o.get("hex").and_then(Json::as_str) {
            Some(text) if o.len() == 1 => Value::Blob(hex::decode(text).map_err(|_| bad())?),
            _ => return Err(bad()),
        },
        _ => return Err(bad()),
    })
}

/// Writes a backup of `store` and `files` into `dir`, which must not exist
/// yet or be empty. Returns the manifest written.
pub async fn backup(
    store: &dyn AsyncStore,
    files: &Files,
    dir: &Path,
    now_ms: i64,
) -> Result<Manifest, BackupError> {
    if dir.exists() && std::fs::read_dir(dir).map_err(io(dir))?.next().is_some() {
        return Err(BackupError(format!(
            "{} is not empty; back up into a new directory",
            dir.display()
        )));
    }
    let tables_dir = dir.join("tables");
    std::fs::create_dir_all(&tables_dir).map_err(io(&tables_dir))?;

    let engine = store.engine();
    let mut tx = store.begin_read().await?;
    let names = tables(&mut *tx, engine).await?;
    if !names.iter().any(|name| name == HISTORY) {
        return Err(BackupError(
            "the database has no Ortus schema; there is nothing to back up".into(),
        ));
    }
    let mut entries = Vec::new();
    for name in &names {
        plain_name(name)?;
        let columns = columns(&mut *tx, engine, name).await?;
        for column in &columns {
            plain_name(column)?;
        }
        let rows = tx
            .query(&format!("SELECT {} FROM {name}", columns.join(", ")), &[])
            .await?;
        let mut text = String::new();
        for row in &rows {
            let line: Vec<Json> = row.0.iter().map(to_json).collect();
            text.push_str(&Json::from(line).to_string());
            text.push('\n');
        }
        let path = tables_dir.join(format!("{name}.jsonl"));
        std::fs::write(&path, &text).map_err(io(&path))?;
        entries.push(TableEntry {
            name: name.clone(),
            columns,
            rows: rows.len() as u64,
            sha256: sha256(text.as_bytes()),
        });
    }
    let history = tx
        .query(
            "SELECT app, MAX(version) FROM ortus_migrations GROUP BY app",
            &[],
        )
        .await?;
    tx.commit().await?;
    let schemas = history
        .iter()
        .filter_map(|row| Some((row.text(0)?.to_string(), row.int(1)?)))
        .collect();

    let mut listing = String::new();
    let (mut count, mut bytes) = (0u64, 0u64);
    for key in files.list("").await? {
        let Some(contents) = files.get(&key).await? else {
            continue; // deleted since it was listed
        };
        let path = dir.join("files").join(&key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::write(&path, &contents).map_err(io(&path))?;
        let line = FileLine {
            key,
            size: contents.len() as u64,
            sha256: sha256(&contents),
        };
        listing.push_str(&serde_json::to_string(&line).map_err(|e| BackupError(e.to_string()))?);
        listing.push('\n');
        count += 1;
        bytes += contents.len() as u64;
    }
    let listing_path = dir.join("files.jsonl");
    std::fs::write(&listing_path, &listing).map_err(io(&listing_path))?;

    let manifest = Manifest {
        format: FORMAT.into(),
        format_version: FORMAT_VERSION,
        ortus_version: crate::VERSION.into(),
        created_at_ms: now_ms,
        engine: engine_name(engine).into(),
        schemas,
        tables: entries,
        files: FilesEntry {
            count,
            bytes,
            sha256: sha256(listing.as_bytes()),
        },
    };
    let path = dir.join("manifest.json");
    let mut text =
        serde_json::to_string_pretty(&manifest).map_err(|e| BackupError(e.to_string()))?;
    text.push('\n');
    // The manifest goes last: a backup without one was interrupted.
    let mut out = std::fs::File::create(&path).map_err(io(&path))?;
    out.write_all(text.as_bytes()).map_err(io(&path))?;
    out.sync_all().map_err(io(&path))?;
    Ok(manifest)
}

/// A backup that has been read and checked, ready to restore.
pub struct Checked {
    pub manifest: Manifest,
    dir: PathBuf,
    files: Vec<FileLine>,
}

/// Reads the backup in `dir` and checks every checksum, and that this
/// program can restore it given the schemas it has.
pub fn check(dir: &Path, schemas: &[Schema]) -> Result<Checked, BackupError> {
    let path = dir.join("manifest.json");
    let text = std::fs::read_to_string(&path).map_err(|e| {
        BackupError(format!(
            "{}: {e}; a backup without a manifest is incomplete",
            path.display()
        ))
    })?;
    let manifest: Manifest = serde_json::from_str(&text)
        .map_err(|e| BackupError(format!("{}: not a valid manifest: {e}", path.display())))?;
    if manifest.format != FORMAT {
        return Err(BackupError(format!(
            "{} is not an Ortus backup",
            dir.display()
        )));
    }
    if manifest.format_version > FORMAT_VERSION {
        return Err(BackupError(format!(
            "the backup is in format {}, newer than this program reads ({FORMAT_VERSION}); restore it with Ortus {} or newer",
            manifest.format_version, manifest.ortus_version
        )));
    }
    for (app, version) in &manifest.schemas {
        let Some(schema) = schemas.iter().find(|schema| schema.app == app) else {
            return Err(BackupError(format!(
                "the backup holds data of '{app}', which this program does not have"
            )));
        };
        if *version > schema.latest() || *version < 1 {
            return Err(BackupError(format!(
                "the backup has '{app}' at version {version}, newer than this program knows ({}); restore it with Ortus {} or newer",
                schema.latest(),
                manifest.ortus_version
            )));
        }
    }
    for table in &manifest.tables {
        plain_name(&table.name)?;
        for column in &table.columns {
            plain_name(column)?;
        }
        let path = dir.join("tables").join(format!("{}.jsonl", table.name));
        let bytes = std::fs::read(&path).map_err(io(&path))?;
        if sha256(&bytes) != table.sha256 {
            return Err(BackupError(format!(
                "{} is damaged: its checksum does not match",
                path.display()
            )));
        }
    }
    let listing_path = dir.join("files.jsonl");
    let listing = std::fs::read(&listing_path).map_err(io(&listing_path))?;
    if sha256(&listing) != manifest.files.sha256 {
        return Err(BackupError(format!(
            "{} is damaged: its checksum does not match",
            listing_path.display()
        )));
    }
    let mut lines = Vec::new();
    for line in listing.lines() {
        let line = line.map_err(io(&listing_path))?;
        let entry: FileLine = serde_json::from_str(&line)
            .map_err(|e| BackupError(format!("{}: {e}", listing_path.display())))?;
        files::check(&entry.key)?;
        let path = dir.join("files").join(&entry.key);
        let bytes = std::fs::read(&path).map_err(io(&path))?;
        if bytes.len() as u64 != entry.size || sha256(&bytes) != entry.sha256 {
            return Err(BackupError(format!(
                "{} is damaged: its checksum does not match",
                path.display()
            )));
        }
        lines.push(entry);
    }
    if lines.len() as u64 != manifest.files.count {
        return Err(BackupError("files.jsonl does not list every file".into()));
    }
    Ok(Checked {
        manifest,
        dir: dir.to_path_buf(),
        files: lines,
    })
}

/// Restores a checked backup into `store` and `files`, which must both be
/// empty.
pub async fn restore(
    checked: &Checked,
    schemas: &[Schema],
    store: &dyn AsyncStore,
    files: &Files,
    now_ms: i64,
) -> Result<(), BackupError> {
    let engine = store.engine();
    {
        let mut tx = store.begin_read().await?;
        let existing = tables(&mut *tx, engine).await?;
        tx.commit().await?;
        let in_use = existing.iter().any(|name| name != HISTORY)
            || (existing.iter().any(|name| name == HISTORY) && !history_is_empty(store).await?);
        if in_use {
            return Err(BackupError(
                "the database is not empty; restore into a new, empty database".into(),
            ));
        }
    }
    if !files.list("").await?.is_empty() {
        return Err(BackupError(
            "the file store is not empty; restore into a new, empty one".into(),
        ));
    }

    // Each schema exactly as it was when the backup was taken.
    for (app, version) in &checked.manifest.schemas {
        let schema = schemas
            .iter()
            .find(|schema| schema.app == app)
            .ok_or_else(|| BackupError(format!("this program does not have '{app}'")))?;
        migrate(store, app, &schema.migrations[..*version as usize], now_ms).await?;
    }

    let mut tx = store.begin().await?;
    let mut target = tables(&mut *tx, engine).await?;
    target.sort();
    let order = load_order(&mut *tx, engine, &target).await?;
    tx.execute("DELETE FROM ortus_migrations", &[]).await?;
    for name in order {
        let Some(table) = checked.manifest.tables.iter().find(|t| t.name == name) else {
            continue;
        };
        let have = columns(&mut *tx, engine, &name).await?;
        if have != table.columns {
            return Err(BackupError(format!(
                "table {name} has columns {:?} here but {:?} in the backup",
                have, table.columns
            )));
        }
        let placeholders: Vec<String> = (1..=have.len()).map(|n| format!("${n}")).collect();
        let sql = format!(
            "INSERT INTO {name} ({}) VALUES ({})",
            have.join(", "),
            placeholders.join(", ")
        );
        let path = checked.dir.join("tables").join(format!("{name}.jsonl"));
        let text = std::fs::read_to_string(&path).map_err(io(&path))?;
        for line in text.lines() {
            let row: Vec<Json> = serde_json::from_str(line)
                .map_err(|e| BackupError(format!("{}: {e}", path.display())))?;
            if row.len() != have.len() {
                return Err(BackupError(format!(
                    "{}: a row has the wrong number of values",
                    path.display()
                )));
            }
            let values = row.iter().map(from_json).collect::<Result<Vec<_>, _>>()?;
            tx.execute(&sql, &values).await?;
        }
    }
    for table in &checked.manifest.tables {
        if !target.contains(&table.name) {
            return Err(BackupError(format!(
                "the backup has a table {} that its schemas do not create",
                table.name
            )));
        }
    }
    tx.commit().await?;

    for entry in &checked.files {
        let path = checked.dir.join("files").join(&entry.key);
        let contents = std::fs::read(&path).map_err(io(&path))?;
        files.put(&entry.key, &contents).await.map_err(|e| {
            BackupError(format!(
                "the database is restored but copying the files failed ({e}); empty both and restore again"
            ))
        })?;
    }
    Ok(())
}

async fn history_is_empty(store: &dyn AsyncStore) -> Result<bool, BackupError> {
    let rows = store
        .query("SELECT COUNT(*) FROM ortus_migrations", &[])
        .await?;
    Ok(rows.first().and_then(|row| row.int(0)) == Some(0))
}
