//! The real program, started with each set of roles: it must open the
//! listeners of its roles and no others.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU32 = AtomicU32::new(0);

struct Program {
    child: Child,
    port: u16,
    dir: PathBuf,
}

impl Drop for Program {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start(roles: &str) -> Program {
    let dir = std::env::temp_dir().join(format!(
        "ortus-core-program-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_ortus-core"))
        .current_dir(&dir)
        .env_clear()
        .env("ORTUS_ROLES", roles)
        .env("ORTUS_DATA_DIR", dir.join("data"))
        .env("ORTUS_WEB_PORT", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    Program { child, port, dir }
}

fn health(port: u16) -> Option<String> {
    get(port, "/api/v1/health")
}

fn get(port: u16, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .ok()?;
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    Some(String::from_utf8_lossy(&response).into_owned())
}

/// Waits until the program has finished starting: its database file is
/// there, which happens before any listener is opened.
fn wait_until_started(program: &mut Program) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !program.dir.join("data/ortus.db").exists() {
        assert!(
            program.child.try_wait().unwrap().is_none(),
            "the program stopped while starting"
        );
        assert!(
            Instant::now() < deadline,
            "the program did not start in 20 s"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_health(program: &mut Program) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(response) = health(program.port).filter(|response| !response.is_empty()) {
            return response;
        }
        assert!(
            program.child.try_wait().unwrap().is_none(),
            "the program stopped while starting"
        );
        assert!(
            Instant::now() < deadline,
            "no answer on port {} in 20 s",
            program.port
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_web_role_serves_http() {
    for roles in ["web", "all", "worker,web"] {
        let mut program = start(roles);
        let response = wait_for_health(&mut program);
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "roles = {roles}: {response}"
        );
        assert!(
            response.contains("\"status\":\"ok\""),
            "roles = {roles}: {response}"
        );
    }
}

#[test]
fn a_program_without_the_web_role_opens_no_http_listener() {
    for roles in ["worker", "mail", "media", "worker,mail,media"] {
        let mut program = start(roles);
        wait_until_started(&mut program);
        // Well past the time a web program needs to start listening.
        std::thread::sleep(Duration::from_millis(1500));
        assert!(
            program.child.try_wait().unwrap().is_none(),
            "roles = {roles}: the program must keep running"
        );
        assert!(
            TcpStream::connect(("127.0.0.1", program.port)).is_err(),
            "roles = {roles}: port {} must be closed",
            program.port
        );
    }
}

#[test]
fn a_wrong_setting_stops_the_program_with_its_name() {
    let mut program = start("web,mial");
    let status = program.child.wait().unwrap();
    let mut message = String::new();
    program
        .child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut message)
        .unwrap();
    assert_eq!(Some(2), status.code());
    assert!(message.contains("unknown role 'mial'"), "{message}");
    assert!(
        !program.dir.join("data").exists(),
        "nothing is created before the settings are checked"
    );
}

#[test]
fn the_database_and_other_files_beside_the_program_are_not_served() {
    let mut program = start("web");
    wait_for_health(&mut program);
    assert!(program.dir.join("data/ortus.db").exists());
    std::fs::write(
        program.dir.join("ortus.toml"),
        "[database]\nurl = \"postgres://ortus:hunter2@db/ortus\"\n",
    )
    .unwrap();
    for path in [
        "/data/ortus.db",
        "/ortus.toml",
        "/data/",
        "/api/../ortus.toml",
        "/assets/../ortus.toml",
        "/assets/%2e%2e/ortus.toml",
    ] {
        let response = get(program.port, path).unwrap_or_default();
        assert!(
            response.starts_with("HTTP/1.1 404") || response.starts_with("HTTP/1.1 400"),
            "{path}: {}",
            response.lines().next().unwrap_or_default()
        );
        assert!(
            !response.contains("hunter2") && !response.contains("SQLite format"),
            "{path} leaked a file"
        );
    }
    let page = get(program.port, "/").unwrap();
    assert!(
        page.starts_with("HTTP/1.1 200") && page.contains("<ortus-shell>"),
        "the web client is served"
    );
}
