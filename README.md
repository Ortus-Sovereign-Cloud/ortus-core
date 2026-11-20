# ortus-core

Ortus core: storage, sign-in, policy, audit, search, admin, public API and web shell.

Part of Ortus, a sovereign workspace for organisations and governments: mail, calendar, files, documents, chat, calls, tasks and approvals that an organisation runs itself.

Status: early development, not ready for use.

## Command line

Every Ortus program has the same commands, and reads the same settings (`ORTUS_CONFIG` or `./ortus.toml`, then `ORTUS_*` variables; see `src/config.rs`):

| Command | What it does |
| --- | --- |
| `serve` (the default) | Runs the program with the roles in its settings. Stops cleanly on SIGTERM; tells systemd when it is ready (`Type=notify`). |
| `backup <dir>` | Writes the database and every file into a new directory while the program keeps running. |
| `check-backup <dir>` | Checks every checksum of a backup and that this program can restore it. |
| `restore <dir>` | Restores a backup into an empty database and file store. |
| `version` | Prints the version. |

A backup does not depend on the engine: one taken from SQLite and a local directory restores into PostgreSQL and S3, and back. See `src/backup.rs` for the format.

## Development

`ci/check.sh` runs every check CI runs. The storage and file tests also run on PostgreSQL and S3 when `ORTUS_TEST_POSTGRES` and `ORTUS_TEST_S3_*` are set (CI requires them).

## Licence

Licensed under either of the Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or the MIT license ([LICENSE-MIT](LICENSE-MIT)), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work, as defined in the Apache-2.0 licence, shall be dual licensed as above, without any additional terms or conditions.
