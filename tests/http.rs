use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct TempData(PathBuf);
impl Drop for TempData {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn start_server(port: u16, data: &TempData) -> Server {
    let child = Command::new(env!("CARGO_BIN_EXE_rustcut"))
        .env("RUSTCUT_ROOT", env!("CARGO_MANIFEST_DIR"))
        .env("RUSTCUT_DATA_DIR", &data.0)
        .env("RUSTCUT_BIND", format!("127.0.0.1:{port}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut server = Server(child);
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return server;
        }
        if server.0.try_wait().unwrap().is_some() {
            panic!("server exited during startup");
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    panic!("server did not start");
}
fn request(port: u16, method: &str, path: &str, body: &str, extra: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut result = Vec::new();
    stream.read_to_end(&mut result).unwrap();
    result
}
fn response_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn edits_ranges_and_saved_project_survive_server_restart() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let data =
        TempData(std::env::temp_dir().join(format!("rustcut-http-{}-{stamp}", std::process::id())));
    let server = start_server(port, &data);
    let initial = response_text(&request(port, "GET", "/api/project", "", ""));
    assert!(initial.contains("200 OK") && initial.contains("\"saved\":false"));
    let range = response_text(&request(
        port,
        "GET",
        "/media/1",
        "",
        "Range: bytes=0-10\r\n",
    ));
    assert!(range.contains("206 Partial Content") && range.contains("Content-Range: bytes 0-10/"));
    let invalid_range = response_text(&request(
        port,
        "GET",
        "/media/1",
        "",
        "Range: bytes=99999999-\r\n",
    ));
    assert!(invalid_range.contains("416 Range Not Satisfiable"));
    let split = response_text(&request(
        port,
        "POST",
        "/api/command",
        "op=split&clip_id=1&at=50",
        "",
    ));
    assert!(split.contains("200 OK") && split.contains("\"created\":5"));
    let overlap = response_text(&request(
        port,
        "POST",
        "/api/command",
        "op=move&clip_id=5&start=49",
        "",
    ));
    assert!(overlap.contains("400 Bad Request"));
    let save = response_text(&request(port, "POST", "/api/save", "", ""));
    assert!(save.contains("200 OK") && save.contains("\"current\":true"));
    drop(server);
    let _server = start_server(port, &data);
    let restored = response_text(&request(port, "GET", "/api/project", "", ""));
    assert!(restored.contains("\"saved\":true"));
    assert!(restored.contains(
        "\"id\":5,\"asset_id\":1,\"track\":\"V1\",\"start\":50,\"source_in\":50,\"duration\":50"
    ));
}
