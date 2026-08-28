//! Write one .m3u per playlist, in the original Spotify order.
//!
//! This is the half Navidrome actually consumes: ND_AUTOIMPORTPLAYLISTS picks
//! the files up from ND_PLAYLISTSPATH on every scan. The PLAYLIST tag is for
//! everything else that reads your library.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Strip the characters that are illegal in a filename on some filesystem or
/// other, so a playlist called "AC/DC <3" still lands on disk.
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "playlist".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Returns how many files were created.
pub fn write_all(dir: &Path, playlists: &[(String, Vec<PathBuf>)]) -> Result<usize> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let mut written = 0;
    for (name, paths) in playlists {
        if paths.is_empty() {
            continue;
        }

        let mut out = String::from("#EXTM3U\n");
        for path in paths {
            // Navidrome — and every other m3u reader — resolves entries
            // relative to the playlist file's own directory, not to the library
            // root. A playlists/ subdir therefore needs "../" prefixes; getting
            // this wrong makes every single entry fail to resolve.
            let rel = pathdiff::diff_paths(path, dir)
                .unwrap_or_else(|| path.clone());
            out.push_str(&rel.to_string_lossy());
            out.push('\n');
        }

        let file = dir.join(format!("{}.m3u", safe_name(name)));
        fs::write(&file, out).with_context(|| format!("writing {}", file.display()))?;
        written += 1;
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_relative_to_the_playlist_dir_not_the_library() {
        let dir = Path::new("/music/playlists");
        let track = PathBuf::from("/music/Ice Cube/01.flac");
        let rel = pathdiff::diff_paths(&track, dir).unwrap();
        assert_eq!(rel, PathBuf::from("../Ice Cube/01.flac"));
    }

    #[test]
    fn safe_name_replaces_path_separators() {
        assert_eq!(safe_name("AC/DC"), "AC_DC");
        assert_eq!(safe_name("   "), "playlist");
    }
}
