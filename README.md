# Glossa

Marginalia & post-it annotations for your book folder.

Point Glossa at a directory of books (PDF first; EPUB/MOBI/DJVU/CBZ are
indexed and slated for later milestones) and annotate them with a visual
layer that lives *around* the text:

- **Post-it notes** — colored cards pinned anywhere on the page.
  Drag to move, double-click to write, 🎨 to cycle colors, 🗑 to delete.
- **Marginalia** — scholar-style notes hanging in a side margin, with a
  connector line drawn to their anchor point on the page edge.
- **Hyperlinks** — link any note to another note, a page of any book
  (by content fingerprint, so links survive file moves), or an external URL.
  Connectors are drawn on the page; navigation is a click away.
- **Annotation sidecars** — every book's annotation layer can be exported
  to a `<book>.glossa.json` next to the book file and re-imported elsewhere.
  Your books are never modified; annotations live in an overlay.

## Run it

```sh
./scripts/fetch-pdfium.sh      # once — installs the pdfium engine
cargo run                      # opens the app
```

Then `📁 Choose folder…`, and pick a directory containing PDFs.

Headless conveniences (useful for scripting/testing without a display):

```sh
glossa --scan <dir>                       # index a folder, print the library
glossa --render <pdf> <page> <out.png>    # render one page as PNG
./scripts/make-test-pdf.py /tmp/x.pdf "Title" 6   # synthetic test PDF
```

## Where things live

| What | Where |
| --- | --- |
| Library database | `~/.local/share/glossa/glossa.db` (SQLite, WAL) |
| pdfium engine | `~/.local/share/glossa/pdfium/libpdfium.so` (or `$GLOSSA_PDFIUM_LIB`) |
| Sidecar files | next to each book: `<name>.glossa.json` |

## Architecture (src/)

- `model.rs` — annotations (`PostIt`/`Margin` anchors, normalized 0..1 page
  coordinates), link targets, book formats
- `store.rs` — SQLite schema: books keyed by sha256 fingerprint, annotations,
  links, thumbnails
- `pdf.rs` — pdfium-render wrapper; holds documents against a global pdfium
- `scan.rs` — background folder scan (size+mtime fast path) + thumbnails
- `sidecar.rs` — `.glossa.json` export / newest-wins import merge
- `app.rs`, `library.rs`, `reader.rs`, `links.rs` — the egui app itself

## Roadmap

- [x] M1 — library scan, PDF rendering, pan/zoom
- [x] M2 — post-its + marginalia (drag, edit, colors, delete)
- [x] M3 — hyperlinks (note↔note, note↔page, note↔URL) + connective drawing
- [x] M4 — sidecar export/import
- [ ] EPUB/MOBI renderers
- [ ] text-selection anchoring for marginalia
- [ ] cross-library graph view
- [ ] exported "stamped" PDF copy
