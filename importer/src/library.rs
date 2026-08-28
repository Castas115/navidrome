//! Index every audio file under the library root.
//!
//! Builds two lookup tables — by ISRC and by normalised `artist|title` — plus
//! the flat key list the fuzzy fallback sweeps.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey};
use walkdir::WalkDir;

use crate::matching;
use crate::model::norm_isrc;

pub const AUDIO_EXT: &[&str] = &["flac", "mp3", "m4a", "mp4", "ogg", "opus", "wav", "aiff", "wv"];

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    /// Seconds. 0 when the format did not report one.
    pub duration_s: f64,
}

#[derive(Debug, Default)]
pub struct Library {
    pub by_isrc: HashMap<String, PathBuf>,
    pub by_key: HashMap<String, Vec<Entry>>,
    /// Deduplicated, insertion-ordered — the fuzzy fallback iterates this.
    pub keys: Vec<String>,
    pub files_seen: usize,
    pub unreadable: usize,
}

struct FileTags {
    artist: String,
    title: String,
    isrc: String,
    duration_s: f64,
}

fn read_tags(path: &Path) -> Option<FileTags> {
    let tagged = match Probe::open(path).and_then(|p| p.read()) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("  unreadable: {} ({err})", path.display());
            return None;
        }
    };

    let duration_s = tagged.properties().duration().as_secs_f64();
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;

    // Fall back to the album artist: compilations and soundtracks often leave
    // the track artist empty.
    let artist = tag
        .artist()
        .or_else(|| tag.get_string(&ItemKey::AlbumArtist).map(Into::into))
        .unwrap_or_default()
        .to_string();
    let title = tag.title().unwrap_or_default().to_string();
    let isrc = norm_isrc(tag.get_string(&ItemKey::Isrc).unwrap_or_default());

    Some(FileTags {
        artist,
        title,
        isrc,
        duration_s,
    })
}

pub fn index(root: &Path) -> Library {
    let mut lib = Library::default();

    let files: Vec<PathBuf> = WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .is_some_and(|e| AUDIO_EXT.contains(&e.as_str()))
        })
        .collect();

    eprintln!("indexing {} files...", files.len());
    lib.files_seen = files.len();

    for path in files {
        let Some(tags) = read_tags(&path) else {
            lib.unreadable += 1;
            continue;
        };
        // A file with no title cannot be matched by anything but ISRC, and an
        // empty key would collide with every other untitled file.
        if tags.title.is_empty() {
            continue;
        }

        // First writer wins: duplicate ISRCs mean duplicate rips.
        if !tags.isrc.is_empty() {
            lib.by_isrc.entry(tags.isrc.clone()).or_insert_with(|| path.clone());
        }

        let key = matching::key(&tags.artist, &tags.title);
        if !lib.by_key.contains_key(&key) {
            lib.keys.push(key.clone());
        }
        lib.by_key.entry(key).or_default().push(Entry {
            path,
            duration_s: tags.duration_s,
        });
    }

    eprintln!(
        "indexed {} unique artist|title keys, {} ISRCs\n",
        lib.keys.len(),
        lib.by_isrc.len()
    );
    lib
}
