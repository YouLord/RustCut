use rustcut::media::MediaTools;
use rustcut::model::{Edit, Project, FPS};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_mp4() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("rustcut-test-{}-{stamp}.mp4", std::process::id()))
}

fn frame_rgb(tools: &MediaTools, path: &Path, seconds: &str) -> Vec<u8> {
    let result = Command::new(&tools.ffmpeg)
        .args(["-v", "error", "-ss", seconds, "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=1:1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(result.status.success());
    result.stdout
}

fn rms(tools: &MediaTools, path: &Path, seconds: &str) -> f64 {
    let result = Command::new(&tools.ffmpeg)
        .args(["-v", "error", "-ss", seconds, "-i"])
        .arg(path)
        .args([
            "-t", "0.5", "-vn", "-ac", "1", "-ar", "48000", "-f", "f32le", "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(result.status.success());
    let samples: Vec<f64> = result
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64)
        .collect();
    assert!(!samples.is_empty());
    (samples.iter().map(|x| x * x).sum::<f64>() / samples.len() as f64).sqrt()
}

#[test]
fn renders_real_mp4_with_gaps_and_split_audio_at_consistent_volume() {
    let tools = MediaTools::discover();
    if !tools.has_ffmpeg || !tools.has_ffprobe {
        eprintln!("Skipping FFmpeg integration test; ffmpeg and ffprobe are required.");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut project = Project::demo(root);
    assert_eq!(project.clips.len(), 4);
    // Leave 2 seconds of black between the first and second video clips.
    project
        .apply(Edit::TrimRight {
            clip_id: 1,
            end: 2 * FPS,
        })
        .unwrap();
    // The audio split creates two input streams that should mix at unity gain.
    project
        .apply(Edit::Split {
            clip_id: 4,
            at: 6 * FPS,
        })
        .unwrap();
    let destination = unique_mp4();
    tools.export(&project, &destination).unwrap();
    let info = tools.probe(&destination, "render.mp4".into()).unwrap();
    assert!(info.has_video && info.has_audio);
    assert_eq!(
        (info.width, info.height, info.duration),
        (1280, 720, 12 * FPS)
    );
    let black = frame_rgb(&tools, &destination, "3");
    assert_eq!(black.len(), 3);
    assert!(
        black.iter().all(|&value| value <= 3),
        "gap should be black: {black:?}"
    );
    let nonblack = frame_rgb(&tools, &destination, "1");
    assert!(
        nonblack.iter().any(|&value| value > 10),
        "clip should contain picture: {nonblack:?}"
    );
    let source = root.join("demo/ambient.mp3");
    for time in ["3", "9"] {
        let source_rms = rms(&tools, &source, time);
        let export_rms = rms(&tools, &destination, time);
        let ratio = export_rms / source_rms;
        assert!(
            (0.75..1.25).contains(&ratio),
            "audio level at {time}s changed: {ratio}"
        );
    }
    let _ = std::fs::remove_file(destination);
}
