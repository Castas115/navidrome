//! The on-disk JSON shape.
//!
//! Deliberately identical to what the original Python `spotify_export.py` and
//! `exportify_to_json.py` emitted, so an existing `playlists.json` still loads
//! and the two implementations can be diffed against each other.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Track {
    pub position: usize,
    pub title: String,
    pub artist: String,
    pub artists: Vec<String>,
    pub album: String,
    pub album_artist: String,
    pub duration_ms: u64,
    pub isrc: String,
    pub spotify_id: String,
    pub is_local: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Playlist {
    pub name: String,
    pub spotify_id: String,
    pub owner: String,
    pub mine: bool,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Export {
    pub playlists: Vec<Playlist>,
}

/// Normalise an ISRC for comparison. Spotify prints them grouped
/// ("US-RC1-23-45678"), local tags usually do not.
pub fn norm_isrc(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}
