//! The one-machine install, end to end with the real program: backup while
//! it runs, restore into a new data directory, a clean stop on SIGTERM and
//! a second start on the same data.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU32 = AtomicU32::new(0);

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir() -> Dir {
    let path = std::env::temp_dir().join(format!(
        "ortus-core-install-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&path).unwrap();
    Dir(path)
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn ortus(data: &Path, port: u16, roles: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ortus-core"));
    command
        .current_dir(data.parent().unwrap())
        .env_clear()
        .env("ORTUS_ROLES", roles)
        .env("ORTUS_DATA_DIR", data)
        .env("ORTUS_WEB_PORT", port.to_string())
        .env("RWS_CONFIG_SHUTDOWN_DELAY_SECS", "0");
    command
}

fn run(data: &Path, args: &[&str]) -> Output {
    ortus(data, free_port(), "all").args(args).output().unwrap()
}

fn get(port: u16, path: &str) -> String {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return String::new();
    }
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    String::from_utf8_lossy(&response).into_owned()
}

struct Running {
    child: Child,
    port: u16,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn serve(data: &Path, roles: &str) -> Running {
    let port = free_port();
    let child = ortus(data, port, roles)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut running = Running { child, port };
    if roles.contains("web") || roles == "all" {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !get(port, "/api/v1/health").starts_with("HTTP/1.1 200") {
            assert!(
                running.child.try_wait().unwrap().is_none(),
                "the program stopped while starting"
            );
            assert!(Instant::now() < deadline, "no health answer in 20 s");
            std::thread::sleep(Duration::from_millis(50));
        }
    } else {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !data.join("ortus.db").exists() {
            assert!(
                Instant::now() < deadline,
                "the program did not start in 20 s"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    assert!(running.child.try_wait().unwrap().is_none());
    running
}

fn terminate(mut running: Running) -> (std::process::ExitStatus, Duration) {
    let started = Instant::now();
    let status = Command::new("kill")
        .args(["-TERM", &running.child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let deadline = started + Duration::from_secs(20);
    loop {
        if let Some(status) = running.child.try_wait().unwrap() {
            return (status, started.elapsed());
        }
        assert!(
            Instant::now() < deadline,
            "the program did not stop on SIGTERM in 20 s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn every_role_stops_cleanly_on_sigterm() {
    for roles in ["all", "web", "worker", "mail,media"] {
        let base = dir();
        let running = serve(&base.0.join("data"), roles);
        let (status, took) = terminate(running);
        assert!(status.success(), "roles = {roles}: {status}");
        assert!(
            took < Duration::from_secs(10),
            "roles = {roles}: took {took:?}"
        );
    }
}

#[test]
fn a_running_program_is_backed_up_and_restored_into_a_new_installation() {
    let base = dir();
    let data = base.0.join("data");
    let running = serve(&data, "all");
    std::fs::create_dir_all(data.join("files/drive")).unwrap();
    std::fs::write(data.join("files/drive/report.txt"), "flood report").unwrap();

    let backup = base.0.join("backup-1");
    let output = run(&data, &["backup", backup.to_str().unwrap()]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("1 files"), "{}", text(&output));
    drop(running);

    let output = run(&data, &["check-backup", backup.to_str().unwrap()]);
    assert!(output.status.success(), "{}", text(&output));

    let other = base.0.join("restored");
    let output = run(&other, &["restore", backup.to_str().unwrap()]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        "flood report",
        std::fs::read_to_string(other.join("files/drive/report.txt")).unwrap()
    );
    // Restoring twice into the same installation is refused.
    let output = run(&other, &["restore", backup.to_str().unwrap()]);
    assert_eq!(Some(1), output.status.code(), "{}", text(&output));
    assert!(text(&output).contains("not empty"), "{}", text(&output));

    // The restored installation starts and serves.
    let running = serve(&other, "all");
    assert!(get(running.port, "/api/v1/health").contains("\"status\":\"ok\""));
}

#[test]
fn a_second_start_on_the_same_data_finds_everything_in_place() {
    let base = dir();
    let data = base.0.join("data");
    for _ in 0..2 {
        let running = serve(&data, "all");
        let (status, _) = terminate(running);
        assert!(status.success());
    }
    let running = serve(&data, "all");
    assert!(get(running.port, "/api/v1/health").contains("\"status\":\"ok\""));
    // The probes Kubernetes uses.
    assert!(get(running.port, "/readyz").starts_with("HTTP/1.1 200"));
    assert!(get(running.port, "/livez").starts_with("HTTP/1.1 200"));
}

#[test]
fn the_command_line_explains_itself() {
    let base = dir();
    let data = base.0.join("data");
    let output = run(&data, &["version"]);
    assert!(output.status.success());
    assert_eq!(
        format!("ortus-core {}\n", env!("CARGO_PKG_VERSION")),
        String::from_utf8_lossy(&output.stdout)
    );
    let output = run(&data, &["help"]);
    assert!(output.status.success());
    assert!(text(&output).contains("backup <dir>"));
    let output = run(&data, &["frobnicate"]);
    assert_eq!(Some(2), output.status.code());
    let output = run(&data, &["backup"]);
    assert_eq!(Some(2), output.status.code());
    let output = run(
        &data,
        &["check-backup", base.0.join("nothing").to_str().unwrap()],
    );
    assert_eq!(Some(1), output.status.code());
    assert!(text(&output).contains("incomplete"), "{}", text(&output));
}
