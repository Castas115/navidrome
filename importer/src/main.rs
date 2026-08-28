//! Turn Exportify playlist exports into a tagged library.
//!
//! Reads the .zip that exportify.net's "Export All" produces (or loose .csv
//! files), matches every exported track against the audio already on disk,
//! then writes:
//!
//!   * one multi-value PLAYLIST tag per file, naming every playlist it is in
//!   * one .m3u per playlist, in the original Spotify order
//!   * playlists.json and unmatched.txt, for debugging a bad match rate
//!
//! No audio is fetched or decoded. This only ever reads and rewrites tags on
//! files that are already in the library.

mod exportify;
mod library;
mod m3u;
mod matching;
mod model;
mod tags;

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use clap::Parser;

use library::{Entry, Library};
use model::{Export, Playlist, Track};

#[derive(Parser, Debug)]
#[command(about, version)]
struct Cli {
    /// Directory watched for Exportify .zip/.csv drops.
    #[arg(long, env = "IMPORT_DIR", default_value = "/import")]
    import_dir: PathBuf,

    /// Root of the music library.
    #[arg(long, env = "MUSIC_DIR", default_value = "/music")]
    music_dir: PathBuf,

    /// Where the .m3u files go. Must be inside the library for Navidrome to see them.
    #[arg(long, env = "PLAYLISTS_DIR", default_value = "/music/playlists")]
    playlists_dir: PathBuf,

    /// Where playlists.json and unmatched.txt are written.
    #[arg(long, env = "DATA_DIR", default_value = "/data")]
    data_dir: PathBuf,

    /// Tag name holding the playlist membership.
    #[arg(long, env = "PLAYLIST_TAG", default_value = "PLAYLIST")]
    tag: String,

    /// Fuzzy match cutoff, 0-100.
    #[arg(long, env = "FUZZY_THRESHOLD", default_value_t = 88.0)]
    threshold: f64,

    /// Seconds of drift allowed when two files share an artist|title.
    #[arg(long, env = "DURATION_TOLERANCE", default_value_t = 7.0)]
    duration_tolerance: f64,

    /// Run once and exit. This is what the Kubernetes CronJob uses.
    #[arg(long, env = "RUN_ONCE")]
    once: bool,

    /// Seconds between polls when watching. Ignored with --once.
    #[arg(long, env = "POLL_INTERVAL", default_value_t = 30)]
    poll_interval: u64,

    /// Match and report, write nothing.
    #[arg(long, env = "DRY_RUN")]
    dry_run: bool,

    /// Only write .m3u files; leave the audio files untouched.
    #[arg(long, env = "NO_TAGS")]
    no_tags: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.once {
        let exports = pending_exports(&cli.import_dir)?;
        if exports.is_empty() {
            eprintln!("nothing in {}, done", cli.import_dir.display());
            return Ok(());
        }
        return run(&cli, &exports);
    }

    watch(&cli)
}

/// Poll the drop directory forever, running the pipeline whenever it fills up.
fn watch(cli: &Cli) -> Result<()> {
    eprintln!(
        "watching {} every {}s",
        cli.import_dir.display(),
        cli.poll_interval
    );
    let interval = Duration::from_secs(cli.poll_interval.max(1));

    loop {
        match pending_exports(&cli.import_dir) {
            Ok(exports) if !exports.is_empty() => {
                // A file that is still being copied in would parse as a
                // truncated zip. Wait for its size to stop moving first.
                if settled(&exports, interval)? {
                    if let Err(err) = run(cli, &exports) {
                        eprintln!("import failed: {err:#}");
                    }
                }
            }
            Ok(_) => {}
            Err(err) => eprintln!("cannot read {}: {err:#}", cli.import_dir.display()),
        }
        std::thread::sleep(interval);
    }
}

/// True once every export has held the same size across two observations.
fn settled(exports: &[PathBuf], interval: Duration) -> Result<bool> {
    let sizes: Vec<u64> = exports
        .iter()
        .map(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .collect();
    std::thread::sleep(interval.min(Duration::from_secs(3)));
    for (path, before) in exports.iter().zip(sizes) {
        let now = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if now != before || now == 0 {
            eprintln!("{} still growing, waiting", path.display());
            return Ok(false);
        }
    }
    Ok(true)
}

fn pending_exports(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .filter(|p| {
            p.extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .is_some_and(|e| e == "zip" || e == "csv")
        })
        .collect();
    found.sort();
    Ok(found)
}

fn run(cli: &Cli, exports: &[PathBuf]) -> Result<()> {
    let names: Vec<String> = exports
        .iter()
        .map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned())
        .collect();
    eprintln!("converting: {}", names.join(", "));

    let playlists = exportify::load(exports)?;
    if playlists.is_empty() {
        eprintln!("no playlists parsed, nothing to do");
        return Ok(());
    }

    fs::create_dir_all(&cli.data_dir)
        .with_context(|| format!("creating {}", cli.data_dir.display()))?;
    let json_path = cli.data_dir.join("playlists.json");
    let export = Export {
        playlists: playlists.clone(),
    };
    fs::write(&json_path, serde_json::to_vec_pretty(&export)?)
        .with_context(|| format!("writing {}", json_path.display()))?;
    let total: usize = playlists.iter().map(|p| p.tracks.len()).sum();
    eprintln!(
        "\n{} playlists, {total} track entries -> {}\n",
        playlists.len(),
        json_path.display()
    );

    let lib = library::index(&cli.music_dir);
    if lib.by_key.is_empty() && lib.by_isrc.is_empty() {
        eprintln!(
            "library at {} is empty — nothing can match yet",
            cli.music_dir.display()
        );
        return Ok(());
    }

    let outcome = resolve(&playlists, &lib, cli);
    report(&outcome, &playlists);

    if cli.dry_run {
        eprintln!("\ndry run — nothing written");
        return Ok(());
    }

    if !cli.no_tags {
        write_tags(&outcome.membership, &cli.tag);
    }

    let ordered: Vec<(String, Vec<PathBuf>)> = playlists
        .iter()
        .map(|p| (p.name.clone(), outcome.resolved.get(&p.name).cloned().unwrap_or_default()))
        .collect();
    let written = m3u::write_all(&cli.playlists_dir, &ordered)?;
    eprintln!("wrote {written} m3u files -> {}", cli.playlists_dir.display());

    write_unmatched(&cli.data_dir, &outcome.missing)?;
    archive(&cli.import_dir, exports)?;
    Ok(())
}

#[derive(Default)]
struct Outcome {
    /// path -> every playlist it belongs to
    membership: HashMap<PathBuf, BTreeSet<String>>,
    /// playlist name -> matched paths, in playlist order
    resolved: HashMap<String, Vec<PathBuf>>,
    /// playlist name -> "artist - title" of everything that missed
    missing: Vec<(String, Vec<String>)>,
    by_method: HashMap<&'static str, usize>,
}

fn resolve(playlists: &[Playlist], lib: &Library, cli: &Cli) -> Outcome {
    let mut out = Outcome::default();

    for pl in playlists {
        let mut hits = Vec::new();
        let mut missed = Vec::new();

        for track in &pl.tracks {
            match match_track(track, lib, cli) {
                Some((path, how)) => {
                    out.membership.entry(path.clone()).or_default().insert(pl.name.clone());
                    hits.push(path);
                    *out.by_method.entry(how).or_default() += 1;
                }
                None => missed.push(format!(
                    "{} - {}",
                    if track.artist.is_empty() { "?" } else { &track.artist },
                    if track.title.is_empty() { "?" } else { &track.title },
                )),
            }
        }

        let total = pl.tracks.len();
        let pct = if total == 0 { 0.0 } else { 100.0 * hits.len() as f64 / total as f64 };
        eprintln!("{}: {}/{total} matched ({pct:.0}%)", pl.name, hits.len());

        out.resolved.insert(pl.name.clone(), hits);
        if !missed.is_empty() {
            out.missing.push((pl.name.clone(), missed));
        }
    }

    out
}

fn match_track(track: &Track, lib: &Library, cli: &Cli) -> Option<(PathBuf, &'static str)> {
    if !track.isrc.is_empty() {
        if let Some(path) = lib.by_isrc.get(&track.isrc) {
            return Some((path.clone(), "isrc"));
        }
    }

    let want_s = track.duration_ms as f64 / 1000.0;
    let pick = |candidates: &[Entry]| -> PathBuf {
        if want_s > 0.0 {
            let nearest = candidates
                .iter()
                .filter(|e| e.duration_s > 0.0 && (e.duration_s - want_s).abs() <= cli.duration_tolerance)
                .min_by(|a, b| {
                    (a.duration_s - want_s)
                        .abs()
                        .total_cmp(&(b.duration_s - want_s).abs())
                });
            if let Some(entry) = nearest {
                return entry.path.clone();
            }
        }
        candidates[0].path.clone()
    };

    let key = matching::key(&track.artist, &track.title);
    if let Some(candidates) = lib.by_key.get(&key) {
        return Some((pick(candidates), "exact"));
    }

    // Fuzzy, but only when the artist half also agrees. Title-only similarity
    // matches far too many covers and same-named songs.
    let (cand_key, _score) = matching::best_match(&key, &lib.keys, cli.threshold)?;
    let artist_score =
        matching::token_sort_ratio(matching::artist_of(&key), matching::artist_of(cand_key));
    if artist_score < 80.0 {
        return None;
    }
    lib.by_key.get(cand_key).map(|c| (pick(c), "fuzzy"))
}

fn report(out: &Outcome, playlists: &[Playlist]) {
    let mut methods: Vec<_> = out.by_method.iter().collect();
    methods.sort();
    let summary: Vec<String> = methods.iter().map(|(k, v)| format!("{k}={v}")).collect();
    eprintln!(
        "\nmatch method: {}",
        if summary.is_empty() { "none".to_string() } else { summary.join(", ") }
    );
    eprintln!(
        "{} unique files across {} playlists",
        out.membership.len(),
        playlists.len()
    );
    let multi = out.membership.values().filter(|v| v.len() > 1).count();
    eprintln!("{multi} files belong to more than one playlist (stored once, tagged with all)");
}

fn write_tags(membership: &HashMap<PathBuf, BTreeSet<String>>, tag_name: &str) {
    let mut failed = 0;
    for (path, names) in membership {
        let values: Vec<String> = names.iter().cloned().collect();
        if let Err(err) = tags::write_playlist_tag(path, &values, tag_name) {
            eprintln!("  tag failed: {} ({err:#})", path.display());
            failed += 1;
        }
    }
    eprintln!(
        "tagged {} files with {tag_name}",
        membership.len() - failed
    );
}

fn write_unmatched(data_dir: &Path, missing: &[(String, Vec<String>)]) -> Result<()> {
    let report = data_dir.join("unmatched.txt");
    if missing.is_empty() {
        let _ = fs::remove_file(&report);
        return Ok(());
    }

    let mut body = String::new();
    let mut total = 0;
    for (name, items) in missing {
        body.push_str(&format!("\n== {name} ({}) ==\n", items.len()));
        for item in items {
            body.push_str(item);
            body.push('\n');
        }
        total += items.len();
    }
    fs::write(&report, body).with_context(|| format!("writing {}", report.display()))?;
    eprintln!("\n{total} unmatched -> {}", report.display());
    Ok(())
}

/// Move the consumed exports aside so the next poll does not reprocess them.
fn archive(import_dir: &Path, exports: &[PathBuf]) -> Result<()> {
    let processed = import_dir.join("processed");
    fs::create_dir_all(&processed)
        .with_context(|| format!("creating {}", processed.display()))?;

    // Seconds since the epoch is enough to keep two exports of the same
    // playlist set from overwriting each other.
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    for path in exports {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let dest = processed.join(format!("{stamp}-{name}"));
        // Across a bind mount rename can fail with EXDEV; fall back to a copy.
        if fs::rename(path, &dest).is_err() {
            fs::copy(path, &dest)
                .with_context(|| format!("copying {} aside", path.display()))?;
            fs::remove_file(path)
                .with_context(|| format!("removing {}", path.display()))?;
        }
    }
    eprintln!("moved {} export(s) to {}", exports.len(), processed.display());
    Ok(())
}
