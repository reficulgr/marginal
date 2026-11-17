//! Folder scanning: index book files into the store and render thumbnails
//! on a background thread, streaming progress + results back to the UI.

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;
use walkdir::WalkDir;

use crate::model::*;
use crate::pdf;
use crate::store::SharedStore;

#[derive(Clone, Debug)]
pub enum ScanMsg {
    /// Scan phase progressed: done / total / current file stem.
    Progress { done: usize, total: usize, current: String },
    /// Thumbnail landed in the DB (UI cares about page_count only).
    Thumb { book_id: i64, page_count: u32 },
    ScanningThumbs,
    Done { errors: Vec<String> },
}

/// SHA-256 of the file contents, streamed.
pub fn fingerprint_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn stat_file(path: &Path) -> Option<(i64, i64)> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some((md.len() as i64, mtime))
}

/// Walk `dir`, index every known book file. Cheap on unchanged files
/// (size+mtime fast path).
pub fn scan_store(store: &SharedStore, dir: &Path, tx: &Sender<ScanMsg>) -> Result<()> {
    let _ = tx.send(ScanMsg::Progress {
        done: 0,
        total: 0,
        current: dir.to_string_lossy().into_owned(),
    });

    // Collect candidates first so total progress is meaningful.
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for entry in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let fmt = BookFormat::from_path(entry.path());
        if fmt.known() {
            candidates.push(entry.path().to_path_buf());
        }
    }
    let total = candidates.len();

    for (i, path) in candidates.iter().enumerate() {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let _ = tx.send(ScanMsg::Progress {
            done: i,
            total,
            current: name.clone(),
        });

        let strpath = path.to_string_lossy().into_owned();
        let Some((size, mtime)) = stat_file(path) else { continue };

        let unchanged = crate::store::with_store(store, |s| s.book_unchanged(&strpath, size, mtime))
            .unwrap_or(false);
        if unchanged {
            continue;
        }

        let fp = match fingerprint_file(path) {
            Ok(fp) => fp,
            Err(e) => {
                errors.push(format!("hashing {strpath}: {e}"));
                continue;
            }
        };
        let title = name.clone();
        let format = BookFormat::from_path(path);

        crate::store::with_store(store, |s| s.upsert_book(&fp, &strpath, &title, format, size, mtime)).ok();
    }

    let _ = tx.send(ScanMsg::ScanningThumbs);
    let errors = render_missing_thumbs(store, tx);

    // Re-send done only once, at the very end.
    let _ = tx.send(ScanMsg::Done { errors });
    Ok(())
}

/// Render thumbnails for PDF books that don't have one yet.
fn render_missing_thumbs(store: &SharedStore, tx: &Sender<ScanMsg>) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    let todos: Vec<(i64, String)> = crate::store::with_store(store, |s| {
        Ok(s.books()?
            .into_iter()
            .filter(|b| b.format.renderable() && !b.has_thumb)
            .map(|b| (b.id, b.path))
            .collect())
    })
    .unwrap_or_default();

    for (id, path) in todos {
        let result = pdf::thumbnail_png(Path::new(&path), 320);
        match result {
            Ok((png, count)) => {
                crate::store::with_store(store, |s| {
                    s.set_thumb(id, &png)?;
                    s.set_page_count(id, count)?;
                    Ok(())
                })
                .ok();
                let _ = tx.send(ScanMsg::Thumb { book_id: id, page_count: count });
            }
            Err(e) => {
                // Broken / encrypted PDFs: leave thumbnail empty, don't fail the scan.
                let _ = tx.send(ScanMsg::Thumb { book_id: id, page_count: 0 });
                errors.push(format!("{path}: {e}"));
            }
        }
    }
    errors
}

/// Spawn the scan on a worker thread. Returns the receiving end.
pub fn spawn_scan(store: SharedStore, dir: PathBuf) -> std::sync::mpsc::Receiver<ScanMsg> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Errors already flow through the channel; final Done marks completion.
        if let Err(e) = scan_store(&store, &dir, &tx) {
            let _ = tx.send(ScanMsg::Done {
                errors: vec![format!("scan failed: {e}")],
            });
        }
    });
    rx
}

/// Poll helper: drains messages with a tiny timeout (not actually used,
/// kept for symmetry; UI uses try_recv in a loop instead).
#[allow(dead_code)]
fn try_drain_timeout(rx: &std::sync::mpsc::Receiver<ScanMsg>) {
    while rx.recv_timeout(Duration::from_millis(0)).is_ok() {}
}
