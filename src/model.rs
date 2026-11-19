//! Data model: books, annotations, links. No egui dependency here.

use serde::{Deserialize, Serialize};
use std::path::Path;

// ---------------------------------------------------------------------------
// Book file formats
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BookFormat {
    Pdf,
    Epub,
    Mobi,
    Azw3,
    Fb2,
    Djvu,
    Cbz,
    Other,
}

impl BookFormat {
    pub fn from_path(p: &Path) -> BookFormat {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "pdf" => BookFormat::Pdf,
            "epub" => BookFormat::Epub,
            "mobi" => BookFormat::Mobi,
            "azw3" | "prc" => BookFormat::Azw3,
            "fb2" => BookFormat::Fb2,
            "djvu" | "djv" => BookFormat::Djvu,
            "cbz" => BookFormat::Cbz,
            _ => BookFormat::Other,
        }
    }

    /// Label shown in the UI.
    pub fn label(self) -> &'static str {
        match self {
            BookFormat::Pdf => "PDF",
            BookFormat::Epub => "EPUB",
            BookFormat::Mobi => "MOBI",
            BookFormat::Azw3 => "AZW3",
            BookFormat::Fb2 => "FB2",
            BookFormat::Djvu => "DJVU",
            BookFormat::Cbz => "CBZ",
            BookFormat::Other => "FILE",
        }
    }

    /// Formats this build can actually open and render (M1: PDF only).
    pub fn renderable(self) -> bool {
        matches!(self, BookFormat::Pdf)
    }

    /// Everything we index in a library folder scan.
    pub fn known(self) -> bool {
        !matches!(self, BookFormat::Other)
    }
}

// ---------------------------------------------------------------------------
// Books
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
#[allow(dead_code)] // author/size/mtime feed future metadata UI
pub struct BookMeta {
    pub id: i64,
    /// content sha256 (hex) — the stable identity of a book, sidecar-friendly
    pub fingerprint: String,
    pub path: String,
    pub title: String,
    pub author: Option<String>,
    pub format: BookFormat,
    pub page_count: Option<u32>,
    pub size_bytes: i64,
    pub mtime: i64,
    pub has_thumb: bool,
}

// ---------------------------------------------------------------------------
// Annotations
// ---------------------------------------------------------------------------

pub type AnnotationId = String; // uuid v4

/// Where an annotation is anchored on a page.
/// Coordinates are normalized (0..1) relative to the *rendered page box*,
/// so annotations survive zooming and resolution changes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Anchor {
    /// Sticky note: a red pin somewhere on the page plus the card hanging
    /// on the desk beside it (may be fully outside the page rectangle).
    PostIt {
        /// pin position, normalized to the page (0..1) — semantic, shared by views
        x: f32,
        y: f32,
        /// user-placed card position in SINGLE-page view (absent = auto-hang)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<[f32; 2]>,
        /// user-placed card position in SPREAD (book) view (absent = auto-hang)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card_s: Option<[f32; 2]>,
        /// user-set card size in px [w, h]; zero/absent = pad default
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<[f32; 2]>,
    },
    /// Marginal note hanging beside the page, at this vertical position.
    Margin { y: f32 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    pub id: AnnotationId,
    pub page: u32, // 1-based
    pub anchor: Anchor,
    pub body: String,
    pub color: String,
    pub created_at: i64,
    pub updated_at: i64,
}

// ---------------------------------------------------------------------------
// Link targets
// ---------------------------------------------------------------------------

// Adjacent tagging: internally-tagged newtype variants holding a bare
// string (Annotation / Url) cannot be serialized by serde at all —
// `{"t": "annotation", "v": "<id>"}` is the stable on-disk and sidecar form.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum LinkTarget {
    /// Another annotation, anywhere in the library.
    Annotation(String),
    /// A page of another book (identified by fingerprint — survives moves).
    Page { book: String, page: u32 },
    /// External resource.
    Url(String),
}

// ---------------------------------------------------------------------------
// Sticky-note colors
// ---------------------------------------------------------------------------

pub const NOTE_COLORS: [(&str, [u8; 3]); 5] = [
    ("yellow", [254, 233, 129]),
    ("pink", [250, 179, 203]),
    ("blue", [167, 213, 245]),
    ("green", [174, 229, 152]),
    ("orange", [250, 197, 142]),
];

/// Lighten a color toward white by fraction t (0..1): paler paper.
pub fn paler(c: [u8; 3], t: f32) -> [u8; 3] {
    [
        (c[0] as f32 + (255.0 - c[0] as f32) * t).min(255.0) as u8,
        (c[1] as f32 + (255.0 - c[1] as f32) * t).min(255.0) as u8,
        (c[2] as f32 + (255.0 - c[2] as f32) * t).min(255.0) as u8,
    ]
}

pub fn note_color_parts(name: &str) -> [u8; 3] {
    NOTE_COLORS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
        .unwrap_or(NOTE_COLORS[0].1)
}

// Returned to duty when the card-colors discussion resumes.
#[allow(dead_code)]
pub fn next_note_color(name: &str) -> String {
    let idx = NOTE_COLORS
        .iter()
        .position(|(n, _)| *n == name)
        .map(|i| (i + 1) % NOTE_COLORS.len())
        .unwrap_or(0);
    NOTE_COLORS[idx].0.to_string()
}

/// A slightly darker variant of a note color, for borders / connectors.
#[allow(dead_code)] // returns with the card-colors pass
pub fn darker_parts(c: [u8; 3], factor: f32) -> [u8; 3] {
    [
        (c[0] as f32 * factor) as u8,
        (c[1] as f32 * factor) as u8,
        (c[2] as f32 * factor) as u8,
    ]
}

// ---------------------------------------------------------------------------
// Bookmark tabs: frosted slip with a colorful tip, sticking on the page's
// top edge. The label runs along the tab; the tip is what a closed book
// shows.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Flag {
    pub id: String,
    pub page: u32, // 1-based
    /// 0..1 along the page width — the tab's centre
    pub x: f32,
    /// a FLAG_COLORS name (the tip)
    pub color: String,
    /// written along the tab; blank = untouched sticker
    pub label: String,
    pub created_at: i64,
    pub updated_at: i64,
}
/// Saturated flag-pack colors — deeper than the note pastels so the tips
/// carry the color story while the body stays frosted.
pub const FLAG_COLORS: [(&str, [u8; 3]); 6] = [
    ("red", [232, 74, 74]),
    ("orange", [245, 145, 55]),
    ("yellow", [242, 200, 56]),
    ("green", [86, 182, 111]),
    ("blue", [70, 150, 225]),
    ("pink", [232, 110, 180]),
];

pub fn flag_rgb(name: &str) -> [u8; 3] {
    FLAG_COLORS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
        .unwrap_or(FLAG_COLORS[0].1)
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod link_target_json_tests {
    use super::*;

    /// All three variants must survive a JSON round trip — this is the seam
    /// that silently refused to serialize (internally-tagged newtype of a
    /// bare string) and made every link insertion fail.
    #[test]
    fn link_targets_round_trip_through_json() {
        for target in [
            LinkTarget::Annotation("8f7b-ann-id".into()),
            LinkTarget::Page { book: "book-fp".into(), page: 12 },
            LinkTarget::Url("https://example.com".into()),
        ] {
            let json = serde_json::to_string(&target).expect("serialize LinkTarget");
            let back: LinkTarget = serde_json::from_str(&json).expect("deserialize LinkTarget");
            assert_eq!(back, target, "round trip failed for {json}");
        }
    }

    #[test]
    fn link_target_json_is_tagged_and_readable() {
        let json = serde_json::to_string(&LinkTarget::Annotation("abc".into())).unwrap();
        assert_eq!(json, r#"{"t":"annotation","v":"abc"}"#);
        let json = serde_json::to_string(&LinkTarget::Page { book: "fp".into(), page: 3 }).unwrap();
        assert_eq!(json, r#"{"t":"page","v":{"book":"fp","page":3}}"#);
    }
}
