use std::path::{Path, PathBuf};

/// The sequence uses integer frames throughout; clip ends are exclusive.
pub const FPS: u32 = 25;
pub const MAX_FRAMES: u32 = FPS * 60 * 60;
pub const MAX_CLIPS: usize = 256;
const HISTORY_LIMIT: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Video,
    Audio,
}

impl Track {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Video => "V1",
            Self::Audio => "A1",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "V1" => Some(Self::Video),
            "A1" => Some(Self::Audio),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    pub path: PathBuf,
    pub thumbnail: Option<PathBuf>,
    pub duration: u32,
    pub has_video: bool,
    pub has_audio: bool,
    pub width: u32,
    pub height: u32,
    /// Normalized source-audio peaks, used only for a visual overview.
    pub waveform: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub id: u64,
    pub asset_id: u64,
    pub track: Track,
    pub start: u32,
    pub source_in: u32,
    pub duration: u32,
}

impl Clip {
    pub fn end(&self) -> u32 {
        self.start.saturating_add(self.duration)
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    pub name: String,
    pub assets: Vec<Asset>,
    pub clips: Vec<Clip>,
    pub revision: u64,
    next_asset_id: u64,
    next_clip_id: u64,
    undo: Vec<Vec<Clip>>,
    redo: Vec<Vec<Clip>>,
}

#[derive(Clone, Copy, Debug)]
pub enum Edit {
    Add {
        asset_id: u64,
        track: Track,
    },
    AddAt {
        asset_id: u64,
        track: Track,
        start: u32,
    },
    Delete {
        clip_id: u64,
    },
    Split {
        clip_id: u64,
        at: u32,
    },
    Move {
        clip_id: u64,
        start: u32,
    },
    TrimLeft {
        clip_id: u64,
        start: u32,
    },
    TrimRight {
        clip_id: u64,
        end: u32,
    },
}

impl Project {
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            assets: Vec::new(),
            clips: Vec::new(),
            revision: 0,
            next_asset_id: 1,
            next_clip_id: 1,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// Seed a self-contained demonstration. Sources are short videos made from
    /// generated stills; the soundtrack is synthesized, not Adobe media.
    pub fn demo(root: &Path) -> Self {
        let mut project = Self::empty("North Atlantic");
        for (name, stem) in [
            ("Coast aerial.mp4", "coast"),
            ("Ocean details.mp4", "shore"),
            ("Ridge walk.mp4", "ridge"),
        ] {
            let path = root.join("demo").join(format!("{stem}.mp4"));
            let thumb = root.join("demo").join(format!("{stem}.jpg"));
            if path.exists() {
                let id = project.insert_asset(Asset {
                    id: 0,
                    name: name.into(),
                    path,
                    thumbnail: thumb.exists().then_some(thumb),
                    duration: 5 * FPS,
                    has_video: true,
                    has_audio: false,
                    width: 960,
                    height: 540,
                    waveform: vec![],
                });
                project.clips.push(Clip {
                    id: project.next_clip_id,
                    asset_id: id,
                    track: Track::Video,
                    start: (id as u32 - 1) * 4 * FPS,
                    source_in: 0,
                    duration: 4 * FPS,
                });
                project.next_clip_id += 1;
            }
        }
        let audio = root.join("demo/ambient.mp3");
        if audio.exists() {
            let id = project.insert_asset(Asset {
                id: 0,
                name: "Ambient texture.mp3".into(),
                path: audio,
                thumbnail: None,
                duration: 15 * FPS,
                has_video: false,
                has_audio: true,
                width: 0,
                height: 0,
                waveform: vec![],
            });
            project.clips.push(Clip {
                id: project.next_clip_id,
                asset_id: id,
                track: Track::Audio,
                start: 0,
                source_in: 0,
                duration: 12 * FPS,
            });
            project.next_clip_id += 1;
        }
        project.revision = 0;
        project
    }

    /// Restore a project after validating IDs, sources and all timeline invariants.
    pub fn from_parts(name: String, assets: Vec<Asset>, clips: Vec<Clip>) -> Result<Self, String> {
        use std::collections::HashSet;
        if name.is_empty() || name.len() > 512 || assets.len() > 1024 || clips.len() > MAX_CLIPS {
            return Err("Некорректный проект".into());
        }
        let mut asset_ids = HashSet::new();
        for asset in &assets {
            if asset.id == 0
                || asset.id == u64::MAX
                || !asset_ids.insert(asset.id)
                || asset.duration == 0
                || asset.duration > MAX_FRAMES
                || (!asset.has_video && !asset.has_audio)
                || asset.waveform.len() > 1000
                || asset.name.len() > 1024
            {
                return Err("Некорректный медиафайл в проекте".into());
            }
        }
        let mut clip_ids = HashSet::new();
        for clip in &clips {
            if clip.id == 0 || clip.id == u64::MAX || !clip_ids.insert(clip.id) {
                return Err("Некорректный клип в проекте".into());
            }
        }
        let next_asset_id = assets.iter().map(|a| a.id).max().unwrap_or(0) + 1;
        let next_clip_id = clips.iter().map(|c| c.id).max().unwrap_or(0) + 1;
        let project = Self {
            name,
            assets,
            clips,
            revision: 0,
            next_asset_id,
            next_clip_id,
            undo: Vec::new(),
            redo: Vec::new(),
        };
        project.validate(&project.clips)?;
        Ok(project)
    }

    pub fn insert_asset(&mut self, mut asset: Asset) -> u64 {
        let id = self.next_asset_id;
        self.next_asset_id += 1;
        asset.id = id;
        self.assets.push(asset);
        self.revision += 1;
        id
    }

    pub fn asset(&self, id: u64) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.id == id)
    }

    pub fn length(&self) -> u32 {
        self.clips.iter().map(Clip::end).max().unwrap_or(0)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> Result<(), String> {
        let previous = self.undo.pop().ok_or("Нечего отменять")?;
        self.redo.push(std::mem::replace(&mut self.clips, previous));
        self.revision += 1;
        Ok(())
    }

    pub fn redo(&mut self) -> Result<(), String> {
        let next = self.redo.pop().ok_or("Нечего повторять")?;
        self.undo.push(std::mem::replace(&mut self.clips, next));
        self.revision += 1;
        Ok(())
    }

    /// All edits are transactional: validate a candidate before committing it.
    pub fn apply(&mut self, edit: Edit) -> Result<Option<u64>, String> {
        let mut candidate = self.clips.clone();
        let mut created = None;
        match edit {
            Edit::Add { asset_id, track }
            | Edit::AddAt {
                asset_id, track, ..
            } => {
                if candidate.len() >= MAX_CLIPS {
                    return Err("Достигнут лимит клипов".into());
                }
                let asset = self.asset(asset_id).ok_or("Медиафайл не найден")?;
                if (track == Track::Video && !asset.has_video)
                    || (track == Track::Audio && !asset.has_audio)
                {
                    return Err("Этот файл нельзя добавить на выбранную дорожку".into());
                }
                let start = match edit {
                    Edit::AddAt { start, .. } => start,
                    _ => candidate
                        .iter()
                        .filter(|c| c.track == track)
                        .map(Clip::end)
                        .max()
                        .unwrap_or(0),
                };
                let id = self.next_clip_id;
                candidate.push(Clip {
                    id,
                    asset_id,
                    track,
                    start,
                    source_in: 0,
                    duration: asset.duration,
                });
                created = Some(id);
            }
            Edit::Delete { clip_id } => {
                let before = candidate.len();
                candidate.retain(|clip| clip.id != clip_id);
                if candidate.len() == before {
                    return Err("Клип не найден".into());
                }
            }
            Edit::Split { clip_id, at } => {
                if candidate.len() >= MAX_CLIPS {
                    return Err("Достигнут лимит клипов".into());
                }
                let clip = candidate
                    .iter_mut()
                    .find(|c| c.id == clip_id)
                    .ok_or("Клип не найден")?;
                if at <= clip.start || at >= clip.end() {
                    return Err("Поставьте курсор внутри выбранного клипа".into());
                }
                let left_duration = at - clip.start;
                let mut right = clip.clone();
                right.id = self.next_clip_id;
                right.start = at;
                right.source_in += left_duration;
                right.duration -= left_duration;
                clip.duration = left_duration;
                created = Some(right.id);
                candidate.push(right);
            }
            Edit::Move { clip_id, start } => {
                let clip = candidate
                    .iter_mut()
                    .find(|c| c.id == clip_id)
                    .ok_or("Клип не найден")?;
                clip.start = start;
            }
            Edit::TrimLeft { clip_id, start } => {
                let clip = candidate
                    .iter_mut()
                    .find(|c| c.id == clip_id)
                    .ok_or("Клип не найден")?;
                let delta = i64::from(start) - i64::from(clip.start);
                let new_in = i64::from(clip.source_in) + delta;
                let new_duration = i64::from(clip.duration) - delta;
                if new_in < 0
                    || new_duration <= 0
                    || new_in > i64::from(u32::MAX)
                    || new_duration > i64::from(u32::MAX)
                {
                    return Err("Граница выходит за пределы исходника".into());
                }
                clip.start = start;
                clip.source_in = new_in as u32;
                clip.duration = new_duration as u32;
            }
            Edit::TrimRight { clip_id, end } => {
                let clip = candidate
                    .iter_mut()
                    .find(|c| c.id == clip_id)
                    .ok_or("Клип не найден")?;
                if end <= clip.start {
                    return Err("Клип должен быть не короче одного кадра".into());
                }
                clip.duration = end - clip.start;
            }
        }
        self.validate(&candidate)?;
        if candidate == self.clips {
            return Err("Нет изменений".into());
        }
        self.undo
            .push(std::mem::replace(&mut self.clips, candidate));
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        if created.is_some() {
            self.next_clip_id += 1;
        }
        self.revision += 1;
        Ok(created)
    }

    pub fn validate(&self, clips: &[Clip]) -> Result<(), String> {
        for clip in clips {
            let asset = self.asset(clip.asset_id).ok_or("Медиафайл не найден")?;
            if clip.duration == 0
                || clip
                    .start
                    .checked_add(clip.duration)
                    .is_none_or(|end| end > MAX_FRAMES)
            {
                return Err("Клип выходит за пределы последовательности (максимум 1 час)".into());
            }
            if clip
                .source_in
                .checked_add(clip.duration)
                .is_none_or(|end| end > asset.duration)
            {
                return Err("Граница выходит за пределы исходника".into());
            }
            if (clip.track == Track::Video && !asset.has_video)
                || (clip.track == Track::Audio && !asset.has_audio)
            {
                return Err("Несовместимая дорожка".into());
            }
        }
        for (index, clip) in clips.iter().enumerate() {
            if clips[index + 1..].iter().any(|other| {
                other.track == clip.track && clip.start < other.end() && other.start < clip.end()
            }) {
                return Err("Клипы на одной дорожке не могут пересекаться".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Project {
        let mut p = Project::empty("Test");
        p.insert_asset(Asset {
            id: 0,
            name: "video".into(),
            path: PathBuf::new(),
            thumbnail: None,
            duration: 250,
            has_video: true,
            has_audio: true,
            width: 640,
            height: 360,
            waveform: vec![],
        });
        p.apply(Edit::Add {
            asset_id: 1,
            track: Track::Video,
        })
        .unwrap();
        p
    }

    #[test]
    fn split_preserves_source_and_timeline_and_undo_redo() {
        let mut p = fixture();
        let id = p
            .apply(Edit::Split { clip_id: 1, at: 75 })
            .unwrap()
            .unwrap();
        let left = p.clips.iter().find(|c| c.id == 1).unwrap();
        let right = p.clips.iter().find(|c| c.id == id).unwrap();
        assert_eq!(
            (left.start, left.source_in, left.duration, left.end()),
            (0, 0, 75, 75)
        );
        assert_eq!(
            (right.start, right.source_in, right.duration, right.end()),
            (75, 75, 175, 250)
        );
        p.undo().unwrap();
        assert_eq!(p.clips.len(), 1);
        p.redo().unwrap();
        assert_eq!(p.clips.len(), 2);
    }

    #[test]
    fn invalid_edit_does_not_mutate_project_or_history() {
        let mut p = fixture();
        let before = p.clips.clone();
        let rev = p.revision;
        assert!(p.apply(Edit::Split { clip_id: 1, at: 0 }).is_err());
        assert!(p
            .apply(Edit::TrimRight {
                clip_id: 1,
                end: 251
            })
            .is_err());
        assert_eq!(p.clips, before);
        assert_eq!(p.revision, rev);
    }

    #[test]
    fn overlap_is_rejected_but_touching_edges_are_allowed() {
        let mut p = fixture();
        let second = p
            .apply(Edit::Split {
                clip_id: 1,
                at: 100,
            })
            .unwrap()
            .unwrap();
        assert!(p
            .apply(Edit::Move {
                clip_id: second,
                start: 99
            })
            .is_err());
        assert!(p
            .apply(Edit::Move {
                clip_id: second,
                start: 120
            })
            .is_ok());
        assert!(p
            .apply(Edit::Move {
                clip_id: second,
                start: 100
            })
            .is_ok());
    }

    #[test]
    fn left_trim_preserves_end_and_cannot_exceed_source() {
        let mut p = fixture();
        p.apply(Edit::TrimLeft {
            clip_id: 1,
            start: 25,
        })
        .unwrap();
        assert_eq!(
            (
                p.clips[0].start,
                p.clips[0].source_in,
                p.clips[0].duration,
                p.clips[0].end()
            ),
            (25, 25, 225, 250)
        );
        p.apply(Edit::TrimLeft {
            clip_id: 1,
            start: 0,
        })
        .unwrap();
        assert_eq!((p.clips[0].source_in, p.clips[0].duration), (0, 250));
        assert!(p
            .apply(Edit::TrimLeft {
                clip_id: 1,
                start: 250
            })
            .is_err());
    }

    #[test]
    fn edits_after_undo_clear_redo() {
        let mut p = fixture();
        p.apply(Edit::TrimRight {
            clip_id: 1,
            end: 100,
        })
        .unwrap();
        p.undo().unwrap();
        assert!(p.can_redo());
        p.apply(Edit::TrimRight {
            clip_id: 1,
            end: 150,
        })
        .unwrap();
        assert!(!p.can_redo());
    }

    #[test]
    fn audio_and_video_tracks_may_overlap() {
        let mut p = fixture();
        p.apply(Edit::Add {
            asset_id: 1,
            track: Track::Audio,
        })
        .unwrap();
        assert_eq!(p.clips.len(), 2);
        assert!(p.validate(&p.clips).is_ok());
    }

    #[test]
    fn positioned_add_must_fit_and_failed_edits_do_not_consume_ids() {
        let mut p = fixture();
        assert!(p
            .apply(Edit::AddAt {
                asset_id: 1,
                track: Track::Video,
                start: 249
            })
            .is_err());
        let id = p
            .apply(Edit::AddAt {
                asset_id: 1,
                track: Track::Video,
                start: 300,
            })
            .unwrap()
            .unwrap();
        assert_eq!(id, 2);
        assert_eq!(p.clips[1].start, 300);
        assert_eq!(p.length(), 550);
    }
}
