use crate::model::{Asset, Clip, Project, Track, FPS, MAX_FRAMES};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct MediaTools {
    pub ffmpeg: OsString,
    pub ffprobe: OsString,
    pub has_ffmpeg: bool,
    pub has_ffprobe: bool,
}

impl MediaTools {
    pub fn discover() -> Self {
        let ffmpeg = std::env::var_os("FFMPEG").unwrap_or_else(|| "ffmpeg".into());
        let ffprobe = std::env::var_os("FFPROBE").unwrap_or_else(|| "ffprobe".into());
        let available = |binary: &OsString| {
            Command::new(binary)
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        Self {
            has_ffmpeg: available(&ffmpeg),
            has_ffprobe: available(&ffprobe),
            ffmpeg,
            ffprobe,
        }
    }

    pub fn probe(&self, path: &Path, name: String) -> Result<Asset, String> {
        if !self.has_ffprobe {
            return Err("Для импорта установите ffprobe (из пакета FFmpeg)".into());
        }
        let output = Command::new(&self.ffprobe)
            .args(["-protocol_whitelist", "file,pipe"])
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(path)
            .output()
            .map_err(|_| "Не удалось запустить ffprobe")?;
        let duration_seconds = String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|s| s.trim().parse::<f64>().ok())
            .filter(|d| d.is_finite() && *d > 0.0 && *d <= f64::from(MAX_FRAMES / FPS))
            .ok_or("Не удалось определить длительность файла, либо она превышает 1 час")?;
        if !output.status.success() {
            return Err("Не удалось прочитать медиафайл".into());
        }
        let duration = (duration_seconds * f64::from(FPS)).floor() as u32;
        if duration == 0 {
            return Err("Медиафайл слишком короткий".into());
        }

        let video = Command::new(&self.ffprobe)
            .args(["-protocol_whitelist", "file,pipe"])
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height",
                "-of",
                "csv=s=x:p=0",
            ])
            .arg(path)
            .output()
            .map_err(|_| "Не удалось определить видеопоток")?;
        let dimensions = String::from_utf8_lossy(&video.stdout);
        let (width, height) = dimensions
            .lines()
            .find_map(|line| {
                let (w, h) = line.trim().split_once('x')?;
                Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
            })
            .unwrap_or((0, 0));
        let audio = Command::new(&self.ffprobe)
            .args(["-protocol_whitelist", "file,pipe"])
            .args([
                "-v",
                "error",
                "-select_streams",
                "a:0",
                "-show_entries",
                "stream=index",
                "-of",
                "csv=p=0",
            ])
            .arg(path)
            .output()
            .map_err(|_| "Не удалось определить аудиопоток")?;
        let has_video = width > 0 && height > 0;
        let has_audio = !String::from_utf8_lossy(&audio.stdout).trim().is_empty();
        if !has_video && !has_audio {
            return Err("В файле нет видео или аудио".into());
        }
        Ok(Asset {
            id: 0,
            name,
            path: path.to_path_buf(),
            thumbnail: None,
            duration,
            has_video,
            has_audio,
            width,
            height,
            waveform: Vec::new(),
        })
    }

    pub fn thumbnail(&self, input: &Path, destination: &Path) -> Option<PathBuf> {
        if !self.has_ffmpeg {
            return None;
        }
        let status = Command::new(&self.ffmpeg)
            .args([
                "-v",
                "error",
                "-y",
                "-ss",
                "0",
                "-protocol_whitelist",
                "file,pipe",
                "-i",
            ])
            .arg(input)
            .args(["-frames:v", "1", "-vf", "scale=320:-2", "-q:v", "5"])
            .arg(destination)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        status.success().then(|| destination.to_path_buf())
    }

    /// A compact, real waveform derived from the audio stream, not fabricated bars.
    pub fn waveform(&self, input: &Path) -> Vec<u8> {
        if !self.has_ffmpeg {
            return Vec::new();
        }
        let output = match Command::new(&self.ffmpeg)
            .args(["-v", "error", "-protocol_whitelist", "file,pipe", "-i"])
            .arg(input)
            .args([
                "-t", "3600", "-map", "0:a:0", "-ac", "1", "-ar", "200", "-f", "s16le", "pipe:1",
            ])
            .stderr(Stdio::null())
            .output()
        {
            Ok(output) if output.status.success() => output,
            _ => return Vec::new(),
        };
        let samples: Vec<u16> = output
            .stdout
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs())
            .collect();
        if samples.is_empty() {
            return Vec::new();
        }
        let buckets = 240.min(samples.len());
        let mut peaks = Vec::with_capacity(buckets);
        for i in 0..buckets {
            let start = i * samples.len() / buckets;
            let end = (i + 1) * samples.len() / buckets;
            peaks.push(*samples[start..end].iter().max().unwrap_or(&0));
        }
        let max = *peaks.iter().max().unwrap_or(&0);
        if max == 0 {
            return vec![0; buckets];
        }
        peaks
            .into_iter()
            .map(|p| ((u32::from(p) * 100) / u32::from(max)) as u8)
            .collect()
    }

    /// Render the validated V1 and A1 tracks to H.264/AAC MP4 using FFmpeg.
    /// Gaps become black/silence. Inputs are passed as individual OS arguments,
    /// never interpolated into a shell command or the filter graph.
    pub fn export(&self, project: &Project, destination: &Path) -> Result<(), String> {
        if !self.has_ffmpeg {
            return Err("Для экспорта установите FFmpeg с кодеком libx264".into());
        }
        project.validate(&project.clips)?;
        let total = project.length();
        if total == 0 {
            return Err("На таймлайне нет клипов".into());
        }
        let mut videos: Vec<&Clip> = project
            .clips
            .iter()
            .filter(|c| c.track == Track::Video)
            .collect();
        let mut audios: Vec<&Clip> = project
            .clips
            .iter()
            .filter(|c| c.track == Track::Audio)
            .collect();
        videos.sort_by_key(|c| c.start);
        audios.sort_by_key(|c| c.start);
        let ordered: Vec<&Clip> = videos.iter().chain(audios.iter()).copied().collect();
        let mut command = Command::new(&self.ffmpeg);
        command.args(["-v", "error", "-y"]);
        for clip in &ordered {
            let asset = project.asset(clip.asset_id).ok_or("Медиафайл не найден")?;
            command
                .args(["-protocol_whitelist", "file,pipe", "-i"])
                .arg(&asset.path);
        }
        let seconds = |f: u32| format!("{:.6}", f64::from(f) / f64::from(FPS));
        let mut filters = Vec::<String>::new();
        let mut video_labels = Vec::<String>::new();
        let mut cursor = 0;
        for (index, clip) in videos.iter().enumerate() {
            if clip.start > cursor {
                let label = format!("gap{}", video_labels.len());
                filters.push(format!(
                    "color=c=black:s=1280x720:r={FPS}:d={}[{label}]",
                    seconds(clip.start - cursor)
                ));
                video_labels.push(label);
            }
            let label = format!("v{index}");
            filters.push(format!("[{index}:v:0]trim=start={}:duration={},setpts=PTS-STARTPTS,scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2,fps={FPS},format=yuv420p,setsar=1[{label}]", seconds(clip.source_in), seconds(clip.duration)));
            video_labels.push(label);
            cursor = clip.end();
        }
        if cursor < total {
            let label = format!("gap{}", video_labels.len());
            filters.push(format!(
                "color=c=black:s=1280x720:r={FPS}:d={}[{label}]",
                seconds(total - cursor)
            ));
            video_labels.push(label);
        }
        let labels = video_labels
            .iter()
            .map(|l| format!("[{l}]"))
            .collect::<String>();
        if video_labels.len() == 1 {
            filters.push(format!("{labels}null[vout]"));
        } else {
            filters.push(format!(
                "{labels}concat=n={}:v=1:a=0[vout]",
                video_labels.len()
            ));
        }
        let mut audio_labels = Vec::new();
        for (index, clip) in ordered.iter().enumerate() {
            let asset = project.asset(clip.asset_id).ok_or("Медиафайл не найден")?;
            if !asset.has_audio {
                continue;
            }
            let label = format!("a{index}");
            let delay = u64::from(clip.start) * 1000 / u64::from(FPS);
            // Pad every stream to the sequence length before mixing. Otherwise
            // amix changes its normalization when a clip ends, altering volume.
            filters.push(format!("[{index}:a:0]atrim=start={}:duration={},asetpts=PTS-STARTPTS,aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,adelay={delay}|{delay},apad,atrim=duration={}[{label}]", seconds(clip.source_in), seconds(clip.duration), seconds(total)));
            audio_labels.push(label);
        }
        if audio_labels.is_empty() {
            filters.push(format!(
                "anullsrc=r=48000:cl=stereo,atrim=duration={}[aout]",
                seconds(total)
            ));
        } else {
            let labels = audio_labels
                .iter()
                .map(|l| format!("[{l}]"))
                .collect::<String>();
            if audio_labels.len() == 1 {
                filters.push(format!("{labels}anull[aout]"));
            } else {
                // Older FFmpeg versions lack amix's `normalize=0` option.
                // All inputs have equal length, so compensating by the number
                // of inputs produces a fixed unity-gain sum on those versions.
                filters.push(format!("{labels}amix=inputs={}:duration=longest:dropout_transition=0,volume={},atrim=duration={}[aout]", audio_labels.len(), audio_labels.len(), seconds(total)));
            }
        }
        let result = command
            .arg("-filter_complex")
            .arg(filters.join(";"))
            .args([
                "-map",
                "[vout]",
                "-map",
                "[aout]",
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "22",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "160k",
                "-movflags",
                "+faststart",
            ])
            .arg(destination)
            .output()
            .map_err(|_| "Не удалось запустить FFmpeg")?;
        if !result.status.success() {
            let _ = std::fs::remove_file(destination);
            let details = String::from_utf8_lossy(&result.stderr);
            return Err(format!(
                "Ошибка FFmpeg: {}",
                details.lines().next().unwrap_or("неизвестная ошибка")
            ));
        }
        Ok(())
    }
}
