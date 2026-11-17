//! Sidecar import/export: `.marginal.json` files that live next to the books.
//! They carry the full annotation layer for one book (including links),
//! so annotations travel with the book when you move or share the file.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::model::*;
use crate::store::Store;

#[derive(Serialize, Deserialize)]
pub struct Sidecar {
    pub format: String,       // "marginal-sidecar"
    pub version: u32,         // 1
    pub source_fingerprint: String,
    pub exported_at: i64,
    pub annotations: Vec<SidecarAnnotation>,
}

#[derive(Serialize, Deserialize)]
pub struct SidecarAnnotation {
    pub id: AnnotationId,
    pub page: u32,
    pub anchor: Anchor,
    pub body: String,
    pub color: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub links: Vec<LinkTarget>,
}

pub fn sidecar_path_for(book_path: &str) -> std::path::PathBuf {
    let book = Path::new(book_path);
    let stem = book
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("book");
    let sidecar_path = PathBuf::from(format!("{stem}.marginal.json"));
    let out: std::path::PathBuf = book
        .parent()
        .map(|p| p.join(&sidecar_path))
        .unwrap_or(sidecar_path);
    out
}

pub fn export(store: &Store, book: &BookMeta) -> Result<std::path::PathBuf> {
    let annotations = store.annotations_for_book(book.id)?;
    let links = store.links_of_book(book.id)?;

    let mut items = Vec::new();
    for ann in &annotations {
        let targets = links
            .iter()
            .filter(|(src, _, _)| src == &ann.id)
            .map(|(_, _, t)| t.clone())
            .collect();
        items.push(SidecarAnnotation {
            id: ann.id.clone(),
            page: ann.page,
            anchor: ann.anchor,
            body: ann.body.clone(),
            color: ann.color.clone(),
            created_at: ann.created_at,
            updated_at: ann.updated_at,
            links: targets,
        });
    }

    let sidecar = Sidecar {
        format: "marginal-sidecar".into(),
        version: 1,
        source_fingerprint: book.fingerprint.clone(),
        exported_at: now_secs(),
        annotations: items,
    };

    let out = sidecar_path_for(&book.path);
    let json = serde_json::to_string_pretty(&sidecar)?;
    std::fs::write(&out, json).with_context(|| format!("writing {}", out.display()))?;
    Ok(out)
}

/// Merge a sidecar into the store for the matching book (by fingerprint).
/// Newest-writer-wins on conflicts (updated_at); links are added if absent.
/// Returns (annotations added, annotations updated, links added).
pub fn import(store: &Store, sidecar_path: &Path) -> Result<(usize, usize, usize)> {
    let raw = std::fs::read_to_string(sidecar_path)
        .with_context(|| format!("reading {}", sidecar_path.display()))?;
    let sidecar: Sidecar = serde_json::from_str(&raw).context("parsing sidecar")?;

    let book = store
        .book_by_fingerprint(&sidecar.source_fingerprint)?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no book in this library has fingerprint {} \
                 — open/import the book file first",
                &sidecar.source_fingerprint[..16.min(sidecar.source_fingerprint.len())]
            )
        })?;

    let mut added = 0;
    let mut updated = 0;
    let mut links_added = 0;

    // Snapshot of the book's current annotations for id lookup.
    let mut snapshot: Vec<Annotation> = store.annotations_for_book(book.id)?;
    let ann_position = |snapshot: &Vec<Annotation>, id: &str| snapshot.iter().position(|a| a.id == id);

    for sc in &sidecar.annotations {
        match ann_position(&snapshot, &sc.id) {
            Some(i) if snapshot[i].updated_at < sc.updated_at => {
                store.update_annotation(
                    &sc.id,
                    &sc.body,
                    &sc.color,
                    sc.page,
                    &sc.anchor,
                    sc.updated_at,
                )?;
                snapshot[i] = Annotation {
                    id: sc.id.clone(),
                    page: sc.page,
                    anchor: sc.anchor,
                    body: sc.body.clone(),
                    color: sc.color.clone(),
                    created_at: sc.created_at,
                    updated_at: sc.updated_at,
                };
                updated += 1;
            }
            Some(_) => {}
            None => {
                let ann = Annotation {
                    id: sc.id.clone(),
                    page: sc.page,
                    anchor: sc.anchor,
                    body: sc.body.clone(),
                    color: sc.color.clone(),
                    created_at: sc.created_at,
                    updated_at: sc.updated_at,
                };
                store.insert_annotation(book.id, &ann)?;
                snapshot.push(ann);
                added += 1;
            }
        }

        let existing_links = store.links_for(&sc.id)?;
        for target in &sc.links {
            if !existing_links.iter().any(|(_, t)| t == target) {
                store.insert_link(&sc.id, target)?;
                links_added += 1;
            }
        }
    }

    Ok((added, updated, links_added))
}
