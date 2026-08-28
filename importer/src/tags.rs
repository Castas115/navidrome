//! Write ONE multi-value tag per file listing every playlist it belongs to.
//!
//! A track in three playlists stays a single file on disk with three values in
//! a single tag, not three copies and not three separate tags.

use std::path::Path;

use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::{ItemKey, ItemValue, Tag, TagItem, TagType};

/// MP4 only accepts a four-character atom or a `----:mean:name` freeform one,
/// so an eight-character key has to be spelled out the long way. Every other
/// container takes the bare name.
pub fn key_for(tag_type: TagType, tag_name: &str) -> ItemKey {
    match tag_type {
        TagType::Mp4Ilst => ItemKey::Unknown(format!("----:com.apple.iTunes:{tag_name}")),
        _ => ItemKey::Unknown(tag_name.to_string()),
    }
}

pub fn write_playlist_tag(path: &Path, values: &[String], tag_name: &str) -> Result<()> {
    let mut tagged = Probe::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .read()
        .with_context(|| format!("parsing {}", path.display()))?;

    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged.primary_tag_mut().expect("inserted above");

    let key = key_for(tag_type, tag_name);
    let _ = tag.remove_key(&key);

    let mut sorted: Vec<&String> = values.iter().collect();
    sorted.sort();
    sorted.dedup();

    // push() silently drops ItemKey::Unknown: it calls map_key() with
    // allow_unknown = false, gets None, and returns false. Every format writer
    // downstream does pass allow_unknown = true, so the item is only unwelcome
    // in this one gatekeeper. Hence push_unchecked throughout.
    if tag_type == TagType::Id3v2 {
        // One item per value would make lofty emit one TXXX frame per value,
        // all sharing a description. ID3v2.4 §4.2.6 allows only one TXXX per
        // description, and readers that enforce that keep a single value.
        // A v2.4 multi-value field is one frame with NUL-separated values.
        let joined = sorted
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\0");
        tag.push_unchecked(TagItem::new(key, ItemValue::Text(joined)));
    } else {
        for value in sorted {
            tag.push_unchecked(TagItem::new(key.clone(), ItemValue::Text(value.clone())));
        }
    }

    tagged
        .save_to_path(path, WriteOptions::default())
        .with_context(|| format!("writing tags to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal ID3v2.4 TXXX reader, straight off the bytes.
    ///
    /// Reading back through lofty would hide the exact bug this guards against:
    /// its `get_user_text` returns a single value whether the file holds one
    /// compliant multi-value frame or several frames illegally sharing a
    /// description. Only the raw bytes tell those apart.
    ///
    /// Panics if more than one TXXX carries `description`, which ID3v2.4 §4.2.6
    /// forbids.
    fn id3v2_user_text(path: &Path, description: &str) -> Vec<String> {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[0..3], b"ID3", "not an ID3v2 tag: {}", path.display());
        assert_eq!(bytes[3], 4, "expected ID3v2.4 for multi-value support");

        let syncsafe = |b: &[u8]| -> usize {
            ((b[0] as usize) << 21) | ((b[1] as usize) << 14) | ((b[2] as usize) << 7) | b[3] as usize
        };

        let end = 10 + syncsafe(&bytes[6..10]);
        let mut pos = 10;
        let mut out = Vec::new();
        let mut matches = 0;

        while pos + 10 <= end {
            let id = &bytes[pos..pos + 4];
            if id == b"\0\0\0\0" {
                break; // padding
            }
            let size = syncsafe(&bytes[pos + 4..pos + 8]);
            let body = &bytes[pos + 10..pos + 10 + size];
            pos += 10 + size;

            if id != b"TXXX" {
                continue;
            }
            assert_eq!(body[0], 3, "test only handles UTF-8 TXXX");
            let rest = &body[1..];
            let nul = rest.iter().position(|&b| b == 0).unwrap();
            if std::str::from_utf8(&rest[..nul]).unwrap() != description {
                continue;
            }
            matches += 1;
            out = std::str::from_utf8(&rest[nul + 1..])
                .unwrap()
                .split('\0')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }

        assert!(
            matches <= 1,
            "{matches} TXXX frames share the description {description:?} in {}; \
             ID3v2.4 allows only one",
            path.display()
        );
        out
    }

    fn read_back(path: &Path, tag_name: &str) -> Vec<String> {
        let tagged = Probe::open(path).unwrap().read().unwrap();
        let tag_type = tagged.primary_tag_type();

        if tag_type == TagType::Id3v2 {
            return id3v2_user_text(path, tag_name);
        }

        let key = key_for(tag_type, tag_name);
        tagged
            .primary_tag()
            .unwrap()
            .get_strings(&key)
            .map(str::to_string)
            .collect()
    }

    /// Private copies, because these tests mutate what they read and cargo
    /// runs them concurrently.
    fn fixtures(scope: &str) -> Vec<std::path::PathBuf> {
        let Ok(dir) = std::env::var("IMPORTER_FIXTURES") else {
            return Vec::new();
        };

        let scratch = std::env::temp_dir().join(format!("navidrome-import-test-{scope}"));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();

        let mut copies = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
            let src = entry.path();
            if !src.is_file() {
                continue;
            }
            let dst = scratch.join(src.file_name().unwrap());
            std::fs::copy(&src, &dst).unwrap();
            copies.push(dst);
        }
        copies.sort();
        assert!(!copies.is_empty(), "no fixtures in {dir}");
        copies
    }

    #[test]
    fn writes_every_value_of_a_multi_value_tag() {
        let values: Vec<String> = ["Gym", "Party", "Road Trip"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        for path in fixtures("write") {
            write_playlist_tag(&path, &values, "PLAYLIST").unwrap();
            assert_eq!(read_back(&path, "PLAYLIST"), values, "{}", path.display());
        }
    }

    /// A track dropped from a playlist has to lose that value, so rewriting
    /// must replace rather than accumulate.
    #[test]
    fn rewriting_replaces_previous_values() {
        let many: Vec<String> = ["Gym", "Party", "Road Trip"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let one = vec!["Solo".to_string()];

        for path in fixtures("rewrite") {
            write_playlist_tag(&path, &many, "PLAYLIST").unwrap();
            write_playlist_tag(&path, &one, "PLAYLIST").unwrap();
            assert_eq!(read_back(&path, "PLAYLIST"), one, "{}", path.display());
        }
    }

    #[test]
    fn mp4_needs_a_freeform_key_but_nothing_else_does() {
        assert_eq!(
            key_for(TagType::Mp4Ilst, "PLAYLIST"),
            ItemKey::Unknown("----:com.apple.iTunes:PLAYLIST".to_string())
        );
        assert_eq!(
            key_for(TagType::VorbisComments, "PLAYLIST"),
            ItemKey::Unknown("PLAYLIST".to_string())
        );
    }
}
