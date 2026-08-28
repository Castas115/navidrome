//! Parse the CSV files that exportify.net produces.
//!
//! "Export All" hands you a zip with one CSV per playlist, named after the
//! playlist. Loose CSVs and a directory of them work too.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};

use crate::model::{norm_isrc, Playlist, Track};

/// Exportify's own headers are "Track Name", "Artist Name(s)", "ISRC" and so
/// on, but the forks drift. Matching on a normalised header means any of these
/// spellings lands in the right slot.
const FIELDS: &[(&str, &[&str])] = &[
    ("title", &["trackname", "name", "title"]),
    ("artist", &["artistnames", "artistname", "artists", "artist"]),
    ("album", &["albumname", "album"]),
    (
        "album_artist",
        &["albumartistnames", "albumartistname", "albumartist"],
    ),
    (
        "duration",
        &["trackdurationms", "durationms", "duration", "tracklength"],
    ),
    ("isrc", &["isrc"]),
    ("uri", &["trackuri", "uri", "trackid", "spotifyid"]),
];

fn norm_header(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn resolve_columns(header: &csv::StringRecord) -> HashMap<&'static str, usize> {
    let seen: HashMap<String, usize> = header
        .iter()
        .enumerate()
        .map(|(i, h)| (norm_header(h), i))
        .collect();

    let mut cols = HashMap::new();
    for (field, candidates) in FIELDS {
        if let Some(idx) = candidates.iter().find_map(|c| seen.get(*c)) {
            cols.insert(*field, *idx);
        }
    }
    cols
}

/// Exportify's column is explicitly "(ms)"; forks sometimes emit seconds or
/// mm:ss instead.
fn duration_ms(raw: &str, header: &str) -> u64 {
    let raw = raw.trim();
    if raw.is_empty() {
        return 0;
    }

    if raw.contains(':') {
        let mut secs = 0.0f64;
        for part in raw.split(':') {
            match part.parse::<f64>() {
                Ok(v) => secs = secs * 60.0 + v,
                Err(_) => return 0,
            }
        }
        return (secs * 1000.0) as u64;
    }

    let Ok(mut val) = raw.parse::<f64>() else {
        return 0;
    };

    // No "ms" in the header and a value too small to be a track length in
    // milliseconds (10s) means the column is really seconds.
    if !norm_header(header).contains("ms") && val < 10_000.0 {
        val *= 1000.0;
    }
    val as u64
}

/// Exportify joins multiple artists with ';' ("Big Pun;Fat Joe"). Older builds
/// and forks use ", " instead, so fall back to the comma only when there is no
/// semicolon — splitting on it unconditionally cuts names like
/// "Tyler, The Creator" in half.
pub fn split_artists(raw: &str) -> Vec<String> {
    let sep = if raw.contains(';') { ';' } else { ',' };
    raw.split(sep)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_csv(name: &str, text: &str) -> Result<Option<Playlist>> {
    // Exportify writes a UTF-8 BOM; csv does not strip it for us.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(text.as_bytes());

    let header = rdr.headers().context("reading header row")?.clone();
    let cols = resolve_columns(&header);

    let Some(&title_col) = cols.get("title") else {
        let sample: Vec<&str> = header.iter().take(6).collect();
        eprintln!(
            "  {name}: no recognisable title column, skipped (headers: {})",
            sample.join(", ")
        );
        return Ok(None);
    };

    let dur_header = cols
        .get("duration")
        .and_then(|i| header.get(*i))
        .unwrap_or("")
        .to_string();

    let mut tracks = Vec::new();
    for record in rdr.records() {
        let record = match record {
            Ok(r) => r,
            // One malformed line should not cost us the whole playlist.
            Err(err) => {
                eprintln!("  {name}: skipping bad row ({err})");
                continue;
            }
        };

        let get = |field: &str| -> &str {
            cols.get(field)
                .and_then(|i| record.get(*i))
                .unwrap_or("")
                .trim()
        };

        let title = record.get(title_col).unwrap_or("").trim();
        if title.is_empty() {
            continue;
        }

        let artists = split_artists(get("artist"));
        let uri = get("uri");

        tracks.push(Track {
            position: tracks.len(),
            title: title.to_string(),
            artist: artists.first().cloned().unwrap_or_default(),
            artists,
            album: get("album").to_string(),
            album_artist: split_artists(get("album_artist"))
                .first()
                .cloned()
                .unwrap_or_default(),
            duration_ms: duration_ms(get("duration"), &dur_header),
            isrc: norm_isrc(get("isrc")),
            spotify_id: uri
                .strip_prefix("spotify:track:")
                .unwrap_or("")
                .to_string(),
            is_local: uri.starts_with("spotify:local:"),
        });
    }

    let with_isrc = tracks.iter().filter(|t| !t.isrc.is_empty()).count();
    eprintln!(
        "  {name}: {} tracks ({with_isrc} with ISRC)",
        tracks.len()
    );

    Ok(Some(Playlist {
        // Exportify sanitises spaces to underscores for the filename; undo that
        // so playlist names (and the .m3u files) read normally.
        name: name.replace('_', " ").trim().to_string(),
        spotify_id: String::new(),
        owner: String::new(),
        mine: true,
        tracks,
    }))
}

/// Yields (playlist name, file contents).
fn sources(inputs: &[impl AsRef<Path>]) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();

    for input in inputs {
        let path = input.as_ref();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();

        if path.is_dir() {
            let mut found: Vec<_> = fs::read_dir(path)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
                })
                .collect();
            found.sort();
            for csv_path in found {
                out.push((stem(&csv_path), fs::read_to_string(&csv_path)?));
            }
        } else if ext == "zip" {
            let file = fs::File::open(path)
                .with_context(|| format!("opening {}", path.display()))?;
            let mut archive = zip::ZipArchive::new(file)
                .with_context(|| format!("reading {} as a zip", path.display()))?;

            let mut names: Vec<String> = (0..archive.len())
                .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
                .filter(|n| n.to_ascii_lowercase().ends_with(".csv"))
                .collect();
            names.sort();

            for name in names {
                let mut buf = Vec::new();
                archive.by_name(&name)?.read_to_end(&mut buf)?;
                // Exportify is UTF-8, but a mojibake byte should not abort the run.
                let text = String::from_utf8_lossy(&buf).into_owned();
                out.push((stem(Path::new(&name)), text));
            }
        } else if ext == "csv" {
            out.push((stem(path), fs::read_to_string(path)?));
        } else {
            eprintln!("skipping {} (not .csv, .zip or a directory)", path.display());
        }
    }

    Ok(out)
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "playlist".to_string())
}

/// Filename order.
pub fn load(inputs: &[impl AsRef<Path>]) -> Result<Vec<Playlist>> {
    let mut playlists = Vec::new();
    let mut without_isrc = 0;

    for (name, text) in sources(inputs)? {
        if let Some(pl) = parse_csv(&name, &text)? {
            if pl.tracks.iter().all(|t| t.isrc.is_empty()) {
                without_isrc += 1;
            }
            playlists.push(pl);
        }
    }

    if without_isrc > 0 {
        // ISRC is the only match that survives a differently-spelled title.
        eprintln!(
            "warning: {without_isrc} playlists had no usable ISRC column — \
             matching falls back to artist/title and will be noticeably worse"
        );
    }

    Ok(playlists)
}
