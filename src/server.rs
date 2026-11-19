use crate::media::MediaTools;
use crate::model::{Edit, Project, Track, FPS};
use crate::persistence;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_UPLOAD: u64 = 100 * 1024 * 1024;
const MAX_HEADER: usize = 16 * 1024;

struct App {
    project: Mutex<Project>,
    tools: MediaTools,
    root: PathBuf,
    data: PathBuf,
    counter: AtomicU64,
    saved_revision: AtomicU64,
    save_lock: Mutex<()>,
    startup_warning: Option<String>,
}

pub fn run(root: PathBuf, bind: &str) -> io::Result<()> {
    let root = root.canonicalize()?;
    let data = std::env::var_os("RUSTCUT_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".rustcut"));
    fs::create_dir_all(data.join("uploads"))?;
    fs::create_dir_all(data.join("exports"))?;
    let data = data.canonicalize()?;
    let tools = MediaTools::discover();
    let (mut project, loaded, startup_warning) = match persistence::load(&root, &data) {
        Ok(Some(project)) => (project, true, None),
        Ok(None) => (Project::demo(&root), false, None),
        Err(error) => {
            eprintln!("Could not load saved project: {error}");
            (
                Project::demo(&root),
                false,
                Some(format!("Не удалось открыть сохранённый проект: {error}")),
            )
        }
    };
    for asset in &mut project.assets {
        if asset.has_audio && asset.waveform.is_empty() {
            asset.waveform = tools.waveform(&asset.path);
        }
    }
    let app = Arc::new(App {
        project: Mutex::new(project),
        tools,
        root,
        data,
        counter: AtomicU64::new(0),
        saved_revision: AtomicU64::new(if loaded { 0 } else { u64::MAX }),
        save_lock: Mutex::new(()),
        startup_warning,
    });
    let listener = TcpListener::bind(bind)?;
    println!(
        "RustCut: http://{bind} (FFmpeg: {}, ffprobe: {})",
        app.tools.has_ffmpeg, app.tools.has_ffprobe
    );
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let app = Arc::clone(&app);
                std::thread::spawn(move || {
                    if let Err(error) = handle(stream, &app) {
                        eprintln!("HTTP: {error}");
                    }
                });
            }
            Err(error) => eprintln!("Connection: {error}"),
        }
    }
    Ok(())
}

fn read_line_limited(reader: &mut BufReader<TcpStream>, limit: usize) -> io::Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_until(b'\n', &mut bytes)?;
    if bytes.len() > limit || (!bytes.is_empty() && !bytes.ends_with(b"\n")) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "header line too long",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid header"))
}

fn handle(stream: TcpStream, app: &App) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    stream.set_write_timeout(Some(Duration::from_secs(60)))?;
    let mut reader = BufReader::new(stream);
    let first = read_line_limited(&mut reader, MAX_HEADER)?;
    if first.is_empty() {
        return Ok(());
    }
    let mut request = first.split_whitespace();
    let method = request.next().unwrap_or("").to_string();
    let target = request.next().unwrap_or("").to_string();
    if !matches!(method.as_str(), "GET" | "HEAD" | "POST") || !target.starts_with('/') {
        return text(
            reader.get_mut(),
            400,
            "Bad Request",
            "text/plain; charset=utf-8",
            b"Bad request",
            &[],
        );
    }
    let mut headers = HashMap::new();
    let mut size = first.len();
    loop {
        let line = read_line_limited(&mut reader, MAX_HEADER.saturating_sub(size))?;
        size += line.len();
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let head = method == "HEAD";
    let range = headers.get("range").map(String::as_str);
    if method == "GET" || head {
        if path == "/api/project" {
            let project = app.project.lock().unwrap();
            let saved = project.revision == app.saved_revision.load(Ordering::Acquire);
            let body = project_json(&project, &app.tools, saved, app.startup_warning.as_deref());
            return text(
                reader.get_mut(),
                200,
                "OK",
                "application/json; charset=utf-8",
                if head { b"" } else { body.as_bytes() },
                &[],
            );
        }
        if let Some(id) = path
            .strip_prefix("/media/")
            .and_then(|s| s.parse::<u64>().ok())
        {
            let asset = app.project.lock().unwrap().asset(id).cloned();
            return match asset {
                Some(asset) => file(
                    reader.get_mut(),
                    &asset.path,
                    media_mime(&asset.path),
                    range,
                    head,
                    false,
                ),
                None => not_found(reader.get_mut()),
            };
        }
        if let Some(id) = path
            .strip_prefix("/thumbnail/")
            .and_then(|s| s.parse::<u64>().ok())
        {
            let thumbnail = app
                .project
                .lock()
                .unwrap()
                .asset(id)
                .and_then(|asset| asset.thumbnail.clone());
            return match thumbnail {
                Some(path) => file(reader.get_mut(), &path, "image/jpeg", range, head, false),
                None => not_found(reader.get_mut()),
            };
        }
        if let Some(name) = path.strip_prefix("/exports/") {
            if let Some(stem) = name.strip_suffix(".mp4") {
                if !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
                    return file(
                        reader.get_mut(),
                        &app.data.join("exports").join(name),
                        "video/mp4",
                        range,
                        head,
                        true,
                    );
                }
            }
            return not_found(reader.get_mut());
        }
        let static_path = match path {
            "/" => Some("web/index.html"),
            "/styles.css" => Some("web/styles.css"),
            "/app.js" => Some("web/app.js"),
            "/fonts/inter-latin-wght-normal.woff2" => {
                Some("web/fonts/inter-latin-wght-normal.woff2")
            }
            "/fonts/inter-cyrillic-wght-normal.woff2" => {
                Some("web/fonts/inter-cyrillic-wght-normal.woff2")
            }
            "/fonts/jetbrains-mono-latin-400-normal.woff2" => {
                Some("web/fonts/jetbrains-mono-latin-400-normal.woff2")
            }
            _ => None,
        };
        return match static_path {
            Some(relative) => file(
                reader.get_mut(),
                &app.root.join(relative),
                media_mime(Path::new(relative)),
                range,
                head,
                false,
            ),
            None => not_found(reader.get_mut()),
        };
    }
    let length = match headers
        .get("content-length")
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(length) => length,
        None => {
            return error_json(
                reader.get_mut(),
                411,
                "Length Required",
                "Не указан размер запроса",
            )
        }
    };
    if path == "/api/import" {
        if length == 0 || length > MAX_UPLOAD {
            return error_json(
                reader.get_mut(),
                413,
                "Payload Too Large",
                "Файл должен быть не больше 100 МБ",
            );
        }
        let params = parse_form(query);
        let name = params.get("name").map(String::as_str).unwrap_or("media");
        let name: String = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("media")
            .chars()
            .filter(|c| !c.is_control())
            .take(120)
            .collect();
        let ext = name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .unwrap_or_default();
        if ![
            "mp4", "mov", "m4v", "mkv", "webm", "mp3", "wav", "m4a", "aac", "ogg", "flac",
        ]
        .contains(&ext.as_str())
        {
            return error_json(
                reader.get_mut(),
                400,
                "Bad Request",
                "Неподдерживаемое расширение файла",
            );
        }
        if !app.tools.has_ffprobe {
            return error_json(
                reader.get_mut(),
                503,
                "Service Unavailable",
                "Для импорта установите ffprobe",
            );
        }
        let unique = unique_id(app);
        let destination = app.data.join("uploads").join(format!("{unique}.{ext}"));
        let mut output = File::create(&destination)?;
        let copied = match io::copy(&mut reader.by_ref().take(length), &mut output) {
            Ok(copied) => copied,
            Err(error) => {
                drop(output);
                let _ = fs::remove_file(&destination);
                return Err(error);
            }
        };
        drop(output);
        if copied != length {
            let _ = fs::remove_file(&destination);
            return error_json(
                reader.get_mut(),
                400,
                "Bad Request",
                "Файл загружен не полностью",
            );
        }
        let mut asset = match app.tools.probe(&destination, name) {
            Ok(asset) => asset,
            Err(error) => {
                let _ = fs::remove_file(&destination);
                return error_json(reader.get_mut(), 400, "Bad Request", &error);
            }
        };
        if asset.has_video {
            asset.thumbnail = app.tools.thumbnail(
                &destination,
                &app.data.join("uploads").join(format!("{unique}.jpg")),
            );
        }
        if asset.has_audio {
            asset.waveform = app.tools.waveform(&destination);
        }
        let mut project = app.project.lock().unwrap();
        if project.assets.len() >= 1024 {
            drop(project);
            let _ = fs::remove_file(&destination);
            if let Some(thumbnail) = asset.thumbnail {
                let _ = fs::remove_file(thumbnail);
            }
            return error_json(
                reader.get_mut(),
                400,
                "Bad Request",
                "Достигнут лимит медиафайлов",
            );
        }
        let id = project.insert_asset(asset);
        drop(project);
        return json(
            reader.get_mut(),
            200,
            "OK",
            &format!("{{\"ok\":true,\"asset_id\":{id}}}"),
        );
    }
    if length > 4096 {
        return error_json(
            reader.get_mut(),
            413,
            "Payload Too Large",
            "Слишком большой запрос",
        );
    }
    let mut body = Vec::with_capacity(length as usize);
    reader.by_ref().take(length).read_to_end(&mut body)?;
    if body.len() != length as usize {
        return error_json(reader.get_mut(), 400, "Bad Request", "Неполный запрос");
    }
    if path == "/api/command" {
        let params = parse_form(&String::from_utf8_lossy(&body));
        let mut project = app.project.lock().unwrap();
        let result = apply_command(&mut project, &params);
        return match result {
            Ok(created) => json(
                reader.get_mut(),
                200,
                "OK",
                &format!(
                    "{{\"ok\":true,\"created\":{}}}",
                    created
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "null".into())
                ),
            ),
            Err(error) => error_json(reader.get_mut(), 400, "Bad Request", &error),
        };
    }
    if path == "/api/save" {
        // Serialize saves, but don't block timeline edits while writing the snapshot.
        let _guard = app.save_lock.lock().unwrap();
        let project = app.project.lock().unwrap().clone();
        return match persistence::save(&project, &app.root, &app.data) {
            Ok(()) => {
                app.saved_revision
                    .store(project.revision, Ordering::Release);
                let current = app.project.lock().unwrap().revision == project.revision;
                json(
                    reader.get_mut(),
                    200,
                    "OK",
                    &format!(
                        "{{\"ok\":true,\"saved_revision\":{},\"current\":{current}}}",
                        project.revision
                    ),
                )
            }
            Err(error) => error_json(reader.get_mut(), 500, "Internal Server Error", &error),
        };
    }
    if path == "/api/export" {
        // Clone the project so edits and preview can continue while FFmpeg runs.
        let project = app.project.lock().unwrap().clone();
        let name = format!("{}.mp4", unique_id(app));
        let path = app.data.join("exports").join(&name);
        return match app.tools.export(&project, &path) {
            Ok(()) => json(
                reader.get_mut(),
                200,
                "OK",
                &format!("{{\"ok\":true,\"url\":\"/exports/{name}\"}}"),
            ),
            Err(error) => error_json(reader.get_mut(), 400, "Bad Request", &error),
        };
    }
    not_found(reader.get_mut())
}

fn unique_id(app: &App) -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{time}-{}", app.counter.fetch_add(1, Ordering::Relaxed))
}

fn apply_command(
    project: &mut Project,
    params: &HashMap<String, String>,
) -> Result<Option<u64>, String> {
    let integer = |name: &str| {
        params
            .get(name)
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| format!("Неверный параметр: {name}"))
    };
    let frame = |name: &str| -> Result<u32, String> {
        u32::try_from(integer(name)?).map_err(|_| format!("Неверный параметр: {name}"))
    };
    let clip = || integer("clip_id");
    match params.get("op").map(String::as_str).unwrap_or("") {
        "add" => {
            let asset_id = integer("asset_id")?;
            let track = params
                .get("track")
                .and_then(|v| Track::parse(v))
                .ok_or("Неверная дорожка")?;
            if params.contains_key("start") {
                project.apply(Edit::AddAt {
                    asset_id,
                    track,
                    start: frame("start")?,
                })
            } else {
                project.apply(Edit::Add { asset_id, track })
            }
        }
        "delete" => project.apply(Edit::Delete { clip_id: clip()? }),
        "split" => project.apply(Edit::Split {
            clip_id: clip()?,
            at: frame("at")?,
        }),
        "move" => project.apply(Edit::Move {
            clip_id: clip()?,
            start: frame("start")?,
        }),
        "trim_left" => project.apply(Edit::TrimLeft {
            clip_id: clip()?,
            start: frame("start")?,
        }),
        "trim_right" => project.apply(Edit::TrimRight {
            clip_id: clip()?,
            end: frame("end")?,
        }),
        "undo" => {
            project.undo()?;
            Ok(None)
        }
        "redo" => {
            project.redo()?;
            Ok(None)
        }
        _ => Err("Неизвестная команда".into()),
    }
}

fn parse_form(value: &str) -> HashMap<String, String> {
    value
        .split('&')
        .filter_map(|pair| {
            if pair.is_empty() {
                return None;
            }
            let (key, val) = pair.split_once('=').unwrap_or((pair, ""));
            Some((decode(key), decode(val)))
        })
        .collect()
}

fn decode(input: &str) -> String {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            };
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(high * 16 + low);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn project_json(
    project: &Project,
    tools: &MediaTools,
    saved: bool,
    warning: Option<&str>,
) -> String {
    let assets = project.assets.iter().map(|a| {
        let waveform = a.waveform.iter().map(u8::to_string).collect::<Vec<_>>().join(",");
        format!("{{\"id\":{},\"name\":{},\"duration\":{},\"has_video\":{},\"has_audio\":{},\"width\":{},\"height\":{},\"url\":\"/media/{}\",\"thumbnail\":{},\"waveform\":[{}]}}",
            a.id, quote(&a.name), a.duration, a.has_video, a.has_audio, a.width, a.height, a.id,
            if a.thumbnail.is_some() { format!("\"/thumbnail/{}\"", a.id) } else { "null".into() }, waveform)
    }).collect::<Vec<_>>().join(",");
    let clips = project.clips.iter().map(|c| format!("{{\"id\":{},\"asset_id\":{},\"track\":\"{}\",\"start\":{},\"source_in\":{},\"duration\":{}}}",
        c.id, c.asset_id, c.track.as_str(), c.start, c.source_in, c.duration)).collect::<Vec<_>>().join(",");
    format!("{{\"name\":{},\"fps\":{},\"width\":1280,\"height\":720,\"length\":{},\"revision\":{},\"saved\":{},\"startup_warning\":{},\"can_undo\":{},\"can_redo\":{},\"ffmpeg\":{},\"ffprobe\":{},\"assets\":[{}],\"clips\":[{}]}}",
        quote(&project.name), FPS, project.length(), project.revision, saved,
        warning.map(quote).unwrap_or_else(|| "null".into()), project.can_undo(), project.can_redo(), tools.has_ffmpeg, tools.has_ffprobe, assets, clips)
}

fn json(stream: &mut TcpStream, status: u16, reason: &str, body: &str) -> io::Result<()> {
    text(
        stream,
        status,
        reason,
        "application/json; charset=utf-8",
        body.as_bytes(),
        &[],
    )
}
fn error_json(stream: &mut TcpStream, status: u16, reason: &str, message: &str) -> io::Result<()> {
    json(
        stream,
        status,
        reason,
        &format!("{{\"error\":{}}}", quote(message)),
    )
}
fn not_found(stream: &mut TcpStream) -> io::Result<()> {
    text(
        stream,
        404,
        "Not Found",
        "text/plain; charset=utf-8",
        b"Not found",
        &[],
    )
}
fn text(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
    extra: &[(&str, String)],
) -> io::Result<()> {
    write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n", body.len())?;
    for (key, value) in extra {
        write!(stream, "{key}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(body)
}

fn file(
    stream: &mut TcpStream,
    path: &Path,
    mime: &str,
    range: Option<&str>,
    head: bool,
    download: bool,
) -> io::Result<()> {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return not_found(stream),
    };
    let size = f.metadata()?.len();
    if size == 0 {
        return text(stream, 200, "OK", mime, b"", &[]);
    }
    let (start, end, partial) = if let Some(range) = range {
        match parse_range(range, size) {
            Some((start, end)) => (start, end, true),
            None => {
                return text(
                    stream,
                    416,
                    "Range Not Satisfiable",
                    "text/plain",
                    b"",
                    &[("Content-Range", format!("bytes */{size}"))],
                )
            }
        }
    } else {
        (0, size - 1, false)
    };
    let length = end - start + 1;
    write!(stream, "HTTP/1.1 {}\r\nContent-Type: {mime}\r\nContent-Length: {length}\r\nAccept-Ranges: bytes\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n", if partial { "206 Partial Content" } else { "200 OK" })?;
    if partial {
        write!(stream, "Content-Range: bytes {start}-{end}/{size}\r\n")?;
    }
    if download {
        write!(
            stream,
            "Content-Disposition: attachment; filename=rustcut-export.mp4\r\n"
        )?;
    }
    stream.write_all(b"\r\n")?;
    if !head {
        f.seek(SeekFrom::Start(start))?;
        io::copy(&mut f.take(length), stream)?;
    }
    Ok(())
}
fn parse_range(range: &str, size: u64) -> Option<(u64, u64)> {
    let range = range.strip_prefix("bytes=")?;
    if range.contains(',') || size == 0 {
        return None;
    }
    let (start, end) = range.split_once('-')?;
    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        return Some((size.saturating_sub(suffix), size - 1));
    }
    let start: u64 = start.parse().ok()?;
    if start >= size {
        return None;
    }
    let end = if end.is_empty() {
        size - 1
    } else {
        end.parse::<u64>().ok()?.min(size - 1)
    };
    (end >= start).then_some((start, end))
}
fn media_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "woff2" => "font/woff2",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_ranges_without_overflow() {
        assert_eq!(parse_range("bytes=0-3", 10), Some((0, 3)));
        assert_eq!(parse_range("bytes=4-", 10), Some((4, 9)));
        assert_eq!(parse_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=-99", 10), Some((0, 9)));
        assert_eq!(parse_range("bytes=10-", 10), None);
        assert_eq!(parse_range("bytes=4-3", 10), None);
        assert_eq!(parse_range("bytes=0-1,4-5", 10), None);
    }
    #[test]
    fn escapes_json_and_decodes_form() {
        assert_eq!(quote("a\"b\n"), "\"a\\\"b\\n\"");
        assert_eq!(
            parse_form("name=%D1%82%D0%B5%D1%81%D1%82+one")
                .get("name")
                .unwrap(),
            "тест one"
        );
        assert_eq!(decode("%я"), "%я");
    }
}
