//! The command line of every Ortus program.
//!
//! ```text
//! ortus [serve]            run the program (the default)
//! ortus backup <dir>       write a backup of the database and files to <dir>
//! ortus backup --into <dir> [--keep N]
//!                          write it to a new dated directory under <dir> and
//!                          keep only the newest N (default 7) there
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
use std::sync::Arc;

use crate::app::App;
use crate::instance::schemas;

/// Exit status for a command that was used wrongly or a bad configuration.
pub const USAGE: i32 = 2;

const HELP: &str =
    "usage: ortus [serve | backup <dir> | check-backup <dir> | restore <dir> | version | help]

  serve               run the program (the default)
  backup <dir>        write a backup of the database and files to a new directory
  backup --into <dir> [--keep N]
                      write it to <dir>/<UTC time> and keep the newest N there (default 7)
  check-backup <dir>  check that a backup is whole and that this program can restore it
  restore <dir>       restore a backup into an empty database and file store
  version             print the version

Settings come from ORTUS_CONFIG (or ./ortus.toml) and ORTUS_* variables.";

/// Runs the command in `args` (without the program name) for a program
/// with these `apps`, and returns the exit status.
pub async fn run(name: &str, version: &str, apps: Vec<Arc<dyn App>>, args: &[String]) -> i32 {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let all_schemas = schemas(&apps);
    let schemas = all_schemas.as_slice();
    match args.as_slice() {
        [] | ["serve"] => with_config(|config| async move {
            crate::program::run(config, apps).await
        })
        .await,
        ["backup", dir] => with_config(|config| async move {
            take_backup(&config, Path::new(dir)).await
        })
        .await,
        ["backup", "--into", parent] | ["backup", "--into", parent, "--keep", _] => {
            let keep = match args.get(4).map(|n| n.parse::<usize>()) {
                None => 7,
                Some(Ok(n)) if n >= 1 => n,
                _ => {
                    eprintln!("ortus: --keep needs a whole number of 1 or more");
                    return USAGE;
                }
            };
            with_config(|config| async move {
                let parent = Path::new(parent);
                let name = dated_name(now_ms());
                let partial = parent.join(format!("{name}.partial"));
                take_backup(&config, &partial).await?;
                std::fs::rename(&partial, parent.join(&name)).map_err(|e| e.to_string())?;
                let removed = prune(parent, keep).map_err(|e| e.to_string())?;
                if removed > 0 {
                    println!("ortus: removed {removed} older backups, keeping {keep}");
                }
                Ok(())
            })
            .await
        }
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

async fn take_backup(config: &Config, dir: &Path) -> Result<(), String> {
    let store = config.open_store().await.map_err(|e| e.to_string())?;
    let files = config.open_files().map_err(|e| e.to_string())?;
    let manifest = backup::backup(store.as_ref(), &files, dir, now_ms())
        .await
        .map_err(|e| e.to_string())?;
    let rows: u64 = manifest.tables.iter().map(|t| t.rows).sum();
    println!(
        "ortus: backup of {} written to {}: {} tables, {rows} rows, {} files ({} bytes)",
        manifest.engine,
        dir.display(),
        manifest.tables.len(),
        manifest.files.count,
        manifest.files.bytes
    );
    Ok(())
}

/// A directory name for a backup taken at `ms`: its UTC time, which sorts
/// in time order, as `20261118T091500Z`.
pub fn dated_name(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Removes all but the newest `keep` finished backups under `parent`, and
/// any interrupted one (`*.partial`). Only directories with a backup name
/// are touched. Returns how many were removed.
pub fn prune(parent: &Path, keep: usize) -> std::io::Result<usize> {
    let is_dated = |name: &str| {
        name.len() == 16
            && name.as_bytes()[8] == b'T'
            && name.ends_with('Z')
            && name[..8].bytes().all(|b| b.is_ascii_digit())
            && name[9..15].bytes().all(|b| b.is_ascii_digit())
    };
    let mut finished = Vec::new();
    let mut removed = 0;
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.strip_suffix(".partial").is_some_and(is_dated) {
            std::fs::remove_dir_all(entry.path())?;
            removed += 1;
        } else if is_dated(&name) {
            finished.push(name);
        }
    }
    finished.sort();
    let surplus = finished.len().saturating_sub(keep);
    for name in &finished[..surplus] {
        std::fs::remove_dir_all(parent.join(name))?;
        removed += 1;
    }
    Ok(removed)
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

#[cfg(test)]
mod tests {
    use super::dated_name;

    #[test]
    fn backup_names_are_utc_times() {
        assert_eq!("19700101T000000Z", dated_name(0));
        // Wednesday 18 November 2026, 09:15:00 UTC.
        assert_eq!("20261118T091500Z", dated_name(1_794_993_300_000));
        assert_eq!("20000229T235959Z", dated_name(951_868_799_000));
    }
}
