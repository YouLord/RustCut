//! A small versioned session format. It is intentionally not Premiere's .prproj.
//! Sources are restricted to the bundled demo and this installation's uploads.
use crate::model::{Asset, Clip, Project, Track};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const FILE_NAME: &str = "project.rustcut";
const MAX_PROJECT_SIZE: u64 = 2 * 1024 * 1024;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn save(project: &Project, root: &Path, data: &Path) -> Result<(), String> {
    project.validate(&project.clips)?;
    let mut output = format!("RUSTCUT\t1\nNAME\t{}\n", hex(project.name.as_bytes()));
    for asset in &project.assets {
        let source = path_token(&asset.path, root, data)?;
        let thumb = match &asset.thumbnail {
            Some(path) => path_token(path, root, data)?,
            None => "-".into(),
        };
        let waveform = asset
            .waveform
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        output.push_str(&format!(
            "ASSET\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            asset.id,
            hex(asset.name.as_bytes()),
            source,
            thumb,
            asset.duration,
            u8::from(asset.has_video),
            u8::from(asset.has_audio),
            asset.width,
            asset.height,
            waveform
        ));
    }
    for clip in &project.clips {
        output.push_str(&format!(
            "CLIP\t{}\t{}\t{}\t{}\t{}\t{}\n",
            clip.id,
            clip.asset_id,
            clip.track.as_str(),
            clip.start,
            clip.source_in,
            clip.duration
        ));
    }
    if output.len() as u64 > MAX_PROJECT_SIZE {
        return Err("Проект слишком большой".into());
    }
    fs::create_dir_all(data).map_err(|e| format!("Не удалось создать папку проекта: {e}"))?;
    let temporary = data.join(format!(
        ".project-{}-{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&temporary, output).map_err(|e| format!("Не удалось сохранить проект: {e}"))?;
    if let Err(error) = fs::rename(&temporary, data.join(FILE_NAME)) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("Не удалось сохранить проект: {error}"));
    }
    Ok(())
}

pub fn load(root: &Path, data: &Path) -> Result<Option<Project>, String> {
    let path = data.join(FILE_NAME);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Не удалось открыть проект: {error}")),
    };
    if metadata.len() > MAX_PROJECT_SIZE {
        return Err("Файл проекта слишком большой".into());
    }
    let content =
        fs::read_to_string(path).map_err(|e| format!("Не удалось прочитать проект: {e}"))?;
    let mut lines = content.lines();
    if lines.next() != Some("RUSTCUT\t1") {
        return Err("Неизвестный формат проекта".into());
    }
    let mut name = None;
    let mut assets = Vec::new();
    let mut clips = Vec::new();
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.first().copied() {
            Some("NAME") if fields.len() == 2 && name.is_none() => name = Some(unhex(fields[1])?),
            Some("ASSET") if fields.len() == 11 => {
                let waveform = if fields[10].is_empty() {
                    Vec::new()
                } else {
                    fields[10]
                        .split(',')
                        .map(|s| {
                            s.parse::<u8>()
                                .map_err(|_| "Повреждённая звуковая волна".to_string())
                        })
                        .collect::<Result<Vec<_>, _>>()?
                };
                assets.push(Asset {
                    id: number(fields[1])?,
                    name: unhex(fields[2])?,
                    path: resolve_token(fields[3], root, data)?,
                    thumbnail: if fields[4] == "-" {
                        None
                    } else {
                        Some(resolve_token(fields[4], root, data)?)
                    },
                    duration: number(fields[5])?,
                    has_video: bit(fields[6])?,
                    has_audio: bit(fields[7])?,
                    width: number(fields[8])?,
                    height: number(fields[9])?,
                    waveform,
                });
            }
            Some("CLIP") if fields.len() == 7 => clips.push(Clip {
                id: number(fields[1])?,
                asset_id: number(fields[2])?,
                track: Track::parse(fields[3]).ok_or("Неизвестная дорожка")?,
                start: number(fields[4])?,
                source_in: number(fields[5])?,
                duration: number(fields[6])?,
            }),
            _ => return Err("Повреждённый файл проекта".into()),
        }
    }
    Ok(Some(Project::from_parts(
        name.ok_or("В проекте нет имени")?,
        assets,
        clips,
    )?))
}

fn number<T: std::str::FromStr>(value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| "Некорректное число в проекте".into())
}
fn bit(value: &str) -> Result<bool, String> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err("Некорректный флаг в проекте".into()),
    }
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 200
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn path_token(path: &Path, root: &Path, data: &Path) -> Result<String, String> {
    for (prefix, directory) in [
        ("demo", root.join("demo")),
        ("upload", data.join("uploads")),
    ] {
        if let Ok(relative) = path.strip_prefix(directory) {
            let name = relative.to_str().ok_or("Неверное имя файла")?;
            if safe_name(name) {
                return Ok(format!("{prefix}/{name}"));
            }
        }
    }
    Err("Медиафайл находится вне папок проекта".into())
}
fn resolve_token(token: &str, root: &Path, data: &Path) -> Result<PathBuf, String> {
    let (prefix, name) = token.split_once('/').ok_or("Неверный путь в проекте")?;
    if !safe_name(name) {
        return Err("Неверный путь в проекте".into());
    }
    match prefix {
        "demo" => Ok(root.join("demo").join(name)),
        "upload" => Ok(data.join("uploads").join(name)),
        _ => Err("Неверный путь в проекте".into()),
    }
}
fn hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}
fn unhex(value: &str) -> Result<String, String> {
    if value.len() % 2 != 0 || !value.is_ascii() || value.len() > 2048 {
        return Err("Некорректный текст в проекте".into());
    }
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digits = std::str::from_utf8(pair).map_err(|_| "Некорректный текст в проекте")?;
            u8::from_str_radix(digits, 16).map_err(|_| "Некорректный текст в проекте")
        })
        .collect::<Result<Vec<_>, _>>()?;
    String::from_utf8(bytes).map_err(|_| "Некорректный текст в проекте".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Edit, FPS};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn save_load_round_trip_preserves_edits_and_unicode() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data =
            std::env::temp_dir().join(format!("rustcut-save-test-{}-{stamp}", std::process::id()));
        let mut project = Project::demo(root);
        project.name = "Тест / монтаж".into();
        project
            .apply(Edit::Split {
                clip_id: 1,
                at: 2 * FPS,
            })
            .unwrap();
        save(&project, root, &data).unwrap();
        let mut loaded = load(root, &data).unwrap().unwrap();
        assert_eq!(loaded.name, project.name);
        assert_eq!(loaded.clips, project.clips);
        assert_eq!(loaded.assets.len(), project.assets.len());
        assert!(!loaded.can_undo());
        let next = loaded
            .apply(Edit::Split {
                clip_id: 2,
                at: 5 * FPS,
            })
            .unwrap()
            .unwrap();
        assert!(next > project.clips.iter().map(|c| c.id).max().unwrap());
        let _ = fs::remove_dir_all(data);
    }

    #[test]
    fn rejects_path_traversal_and_duplicate_ids() {
        let root = Path::new("/test/root");
        let data = Path::new("/test/data");
        assert!(resolve_token("upload/../../secret", root, data).is_err());
        assert!(resolve_token("demo/..", root, data).is_err());
        let mut project = Project::demo(Path::new(env!("CARGO_MANIFEST_DIR")));
        project.assets.push(project.assets[0].clone());
        assert!(Project::from_parts(project.name, project.assets, project.clips).is_err());
    }
}
