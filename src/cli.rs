//! The command line of every Ortus program.
//!
//! ```text
//! ortus [serve]            run the program (the default)
//! ortus backup <dir>       write a backup of the database and files to <dir>
//! ortus check-backup <dir> check a backup without restoring it
//! ortus restore <dir>      restore a backup into an empty installation
//! ortus version            print the version
//! ```
//!
//! Every command reads the same configuration as the program itself, so a
//! backup is taken of exactly the installation the program serves.

use std::path::Path;

use crate::backup;
use crate::config::Config;
use crate::program::now_ms;
use crate::storage::Schema;

/// Exit status for a command that was used wrongly or a bad configuration.
pub const USAGE: i32 = 2;

const HELP: &str =
    "usage: ortus [serve | backup <dir> | check-backup <dir> | restore <dir> | version | help]

  serve               run the program (the default)
  backup <dir>        write a backup of the database and files to a new directory
  check-backup <dir>  check that a backup is whole and that this program can restore it
  restore <dir>       restore a backup into an empty database and file store
  version             print the version

Settings come from ORTUS_CONFIG (or ./ortus.toml) and ORTUS_* variables.";

/// Runs the command in `args` (without the program name) for a program
/// with these `schemas`, and returns the exit status.
pub async fn run(name: &str, version: &str, schemas: &[Schema], args: &[String]) -> i32 {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] | ["serve"] => with_config(|config| async move {
            crate::program::run(config, schemas).await
        })
        .await,
        ["backup", dir] => with_config(|config| async move {
            let store = config.open_store().await.map_err(|e| e.to_string())?;
            let files = config.open_files().map_err(|e| e.to_string())?;
            let manifest = backup::backup(store.as_ref(), &files, Path::new(dir), now_ms())
                .await
                .map_err(|e| e.to_string())?;
            let rows: u64 = manifest.tables.iter().map(|t| t.rows).sum();
            println!(
                "ortus: backup written to {dir}: {} tables, {rows} rows, {} files ({} bytes)",
                manifest.tables.len(),
                manifest.files.count,
                manifest.files.bytes
            );
            Ok(())
        })
        .await,
        ["check-backup", dir] => match backup::check(Path::new(dir), schemas) {
            Ok(checked) => {
                let m = &checked.manifest;
                println!(
                    "ortus: {dir} is a whole backup from Ortus {} ({}), {} tables, {} files; this program can restore it",
                    m.ortus_version,
                    m.engine,
                    m.tables.len(),
                    m.files.count
                );
                0
            }
            Err(e) => {
                eprintln!("ortus: {e}");
                1
            }
        },
        ["restore", dir] => with_config(|config| async move {
            let checked = backup::check(Path::new(dir), schemas).map_err(|e| e.to_string())?;
            let store = config.open_store().await.map_err(|e| e.to_string())?;
            let files = config.open_files().map_err(|e| e.to_string())?;
            backup::restore(&checked, schemas, store.as_ref(), &files, now_ms())
                .await
                .map_err(|e| e.to_string())?;
            println!(
                "ortus: restored {dir} (taken by Ortus {}); start the program to finish any upgrade",
                checked.manifest.ortus_version
            );
            Ok(())
        })
        .await,
        ["version" | "--version" | "-V"] => {
            println!("{name} {version}");
            0
        }
        ["help" | "--help" | "-h"] => {
            println!("{HELP}");
            0
        }
        _ => {
            eprintln!("{HELP}");
            USAGE
        }
    }
}

async fn with_config<F, Fut>(command: F) -> i32
where
    F: FnOnce(Config) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let config = match Config::from_process() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("ortus: {e}");
            return USAGE;
        }
    };
    match command(config).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ortus: {e}");
            1
        }
    }
}
