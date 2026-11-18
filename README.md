# Marginal

*The margins are the point.*

Marginalia, post-it notes and hyperlinks for your book folder.

Point Marginal at a directory of books (PDF today; EPUB/MOBI/DJVU/CBZ are
indexed and slated for later milestones) and annotate with a visual layer
that lives *around* the text:

- **Post-it notes** — analog paper cards pinned anywhere on the page by a red
  pin, hanging on the dot-grid desk. Hand-tilted silhouettes, dog-ear-free
  since v2, paper grain, feathered gaussian shadows, elastic ink height,
  resizable with a 15-characters × 7-lines floor.
- **Marginalia** — white chalk handwriting (Rock Salt) floating beside the
  page, its dashed thread crossing the sheet to underline the exact passage.
  The anchor dot is draggable to re-target. The margin is a solved column:
  when notes would overlap (deep zoom-out), a clashing note reflows wider
  into the free desk — fewer rows, shorter box, top still pinned at its
  dot — and whatever still collides slides down the strip, hanging from
  its dot by its thread. Single-row notes are pins: never widened, never
  moved, never overlapped — the solve stops at them. Judged on measured
  text every frame from the notes' original widths (never written back),
  so the arrangement reverts by itself as the crowding goes away.
- **Installing** — `cargo run` is the dev loop; to keep Marginal, run
  `pkgbuild/install.sh`: it builds the release binary and installs it with
  a launcher entry and icon into `~/.local` (binary on your PATH at
  `~/.local/bin/marginal`, opens from the app launcher). A system-wide
  `makepkg` install uses the same files staged into `pkgbuild/src/`. Fonts
  are compiled in and state lives in `~/.local/share/marginal`, so the
  binary is portable.
- **Hyperlinks** — link any note to another note, any page of any book (by
  content fingerprint, so links survive file moves), or an external URL.
  Hand-drawn dashed threads, drawn across spreads when both notes are open.
  In the notes strip each connection is listed right under its note — click
  it to jump (across books too; URLs open in the browser), ✕ to remove just
  the link. The desk hover bar grows one `→` per link to jump from there.
- **The notes strip** — fixed sidebar UI: 20%-transparent Midnight laid
  over the full-width desk. Pages, notes and the dot grid show through
  the translucency and pan beneath it like anywhere else, but the strip's
  own contents respond to nothing — no wheel, no content drag, only the
  scrollbar — so the sidebar itself never moves. Soft drop shadow and a
  wisp of emulsion grain. Every annotation renders as it appears on the
  desk — paper tiles, chalk handwriting — grouped by page, in reading
  order, connections listed directly under their note.
- **Book view** — facing pages with a binding gutter: left = even, right =
  odd, title page alone on the right. ◀ ▶ flip spread by spread. Marginalia
  hangs on the outer margins; the spine stays clean. Single-view and
  book-view keep separate, independent card arrangements.
- **The ruler** — a transparent strip floating over the desk: tick marks
  every 2% of the page span, chapter skeleton in amber (from the PDF's
  embedded outline), a red hairline marker for your position, click /
  drag-to-scrub navigation, and `p.N · Chapter` hover previews. Pages that
  hold a post-it show the pin's exact red on the baseline; pages with
  marginalia float the margin dot's light blue just above it — one dot per
  page, so the ruler reads as a census of where the notes live.
- **Pannable desk** — hold space (or middle-drag) to move everything around;
  ctrl+wheel zooms; the wheel pans.
- **Notes index strip** — every post-it and marginalia in one right-hand panel, ordered by where they sit in the book (page, then reading position). Post-its render as the same paper tiles as on the desk, marginalia as the same chalk; click any entry to jump to its page. New pins land in the strip in book order. Right-click any note — on the desk or in the strip — to duplicate it (same ink, fresh pin spot) or delete it (its links go with it).
- **Sidecars** — export any book's annotation layer to `<book>.marginal.json`
  next to the book file and re-import it elsewhere. Books are never modified;
  annotations live in an overlay.

## Run it

```sh
./scripts/fetch-pdfium.sh      # once — installs the pdfium engine
cargo run                      # opens the app
```

Then `📁 Choose folder…`, and pick a directory containing PDFs.

Headless conveniences (useful for scripting/testing without a display):

```sh
marginal --scan <dir>                       # index a folder, print results
marginal --render <pdf> <page> <out.png>    # render one page as PNG
./scripts/make-test-pdf.py /tmp/x.pdf "Title" 6   # synthetic test PDF
```

## Where things live

| What | Where |
| --- | --- |
| Library database | `~/.local/share/marginal/marginal.db` (SQLite, WAL) |
| pdfium engine | `~/.local/share/marginal/pdfium/libpdfium.so` (or `$MARGINAL_PDFIUM_LIB`) |
| Sidecar files | next to each book: `<name>.marginal.json` |

Pre-rename installs migrate automatically: the first launch moves
`~/.local/share/glossa/` data into place, and the legacy pdfium path remains
a fallback.

## Architecture (src/)

- `model.rs` — annotations (`PostIt`/`Margin` anchors, normalized 0..1 page
  coordinates, per-view-mode card positions), link targets, book formats
- `store.rs` — SQLite schema: books keyed by sha256 fingerprint, annotations,
  links, thumbnails, view preferences
- `pdf.rs` — pdfium-render wrapper; a global pdfium engine, page rendering
  and outline (chapter) extraction
- `scan.rs` — background folder scan (size+mtime fast path) + thumbnails
- `sidecar.rs` — `.marginal.json` export / newest-wins import merge
- `theme.rs` — charcoal desk, dot grid, fonts (Rock Salt note face)
- `app.rs`, `library.rs`, `reader.rs`, `links.rs` — the egui app itself

Tests: `cargo test` (headless egui interaction harness in
`tests/postit_flow.rs`).

## Roadmap

- [x] M1 — library scan, PDF rendering, pan/zoom
- [x] M2 — post-its + marginalia (drag, edit, colors, delete)
- [x] M3 — hyperlinks (note↔note, note↔page, note↔URL) + connective drawing
- [x] M4 — sidecar export/import
- [x] M5 — book (facing-pages) view with per-mode arrangement memory
- [x] M6 — the ruler: 2% ticks, chapter skeleton, scrub navigation
- [ ] EPUB/MOBI renderers
- [ ] text-selection anchoring for marginalia
- [ ] cross-library graph view
- [ ] exported "stamped" PDF copy
