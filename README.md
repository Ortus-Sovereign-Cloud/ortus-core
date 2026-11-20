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

## Apps

An app (mail, drive, chat, ...) is a value implementing `ortus_core::app::App`. It declares its switcher entry (id, group, path), its tables as migrations, its API routes under `/api/v1/<id>/`, its kinds of background job (`<id>.<kind>`), the tasks it runs for a role (SMTP for the mail role) and the services it offers other apps. A program is the core plus a list of apps: `ortus_core::cli::run(name, version, apps, args)`.

Apps never depend on each other. What one may ask of another is a trait in `ortus_core::interfaces` (Calendar, Contacts, Drive, Chat, Tasks); the app that can do it provides it, and the asker gets `NotInstalled` when that app is absent or off.

Every app has a switch (`Instance::switches`): off by default when listed in `[apps] off` (or `ORTUS_APPS_OFF`), and an administrator's choice in the database wins. A switched-off app leaves the switcher, its API answers `not_installed`, its jobs wait in the queue and its role tasks are not started at the next start; its data stays. The web client shows "not installed" for an app path that has no app.

## Development

`ci/check.sh` runs every check CI runs. The storage and file tests also run on PostgreSQL and S3 when `ORTUS_TEST_POSTGRES` and `ORTUS_TEST_S3_*` are set (CI requires them).

## Licence

Licensed under either of the Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or the MIT license ([LICENSE-MIT](LICENSE-MIT)), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work, as defined in the Apache-2.0 licence, shall be dual licensed as above, without any additional terms or conditions.
