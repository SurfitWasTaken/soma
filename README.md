# Soma

A local-first, native Rust PDF reader whose annotations are a typed graph. See [PRD.md](PRD.md).

The core idea is that relations are first-class: a relation between two nodes can have its own title and body, be filed into a system, and be the endpoint of another relation. None of that changes the graph's topology. `A—B` stays a single relation (path length 1) whether it is bare, annotated or promoted.

## Status

This build covers milestones **M0–M4** from the PRD: the reader, capture, the canvas and reification. Of the M5/M6 work, only a few pieces are here: overlay composition, ghost mode, re-anchoring on relink and JSON export. Saved views, the friction and abstraction lenses, Markdown/GraphML export and packaging are not built yet.

| Acceptance test | Where it's verified |
|---|---|
| A1 filed, highlighted, anchored node in 2 keystrokes | UI self-test (`SOMA_AUTOTEST`) |
| A2 link `prerequisite-of` in 3 keystrokes | UI self-test |
| A3 relation annotated, filed, targeted; path stays 1 | `soma-core/tests/core.rs`, UI self-test |
| A4 cascade delete, one undo restores it all | `soma-core` (incl. property tests), `soma-store`, UI self-test |
| A5 ≥95% re-anchored at ≥0.9 after a revision | `soma-pdf/tests/pdfium.rs` (real PDFium, reflowed revision) |
| A6 overlay toggle < 33 ms at 10k/20k | `soma-cli bench` (~6 ms release); the 60 fps pan/zoom half is not measured |
| A7 SIGKILL loses at most the in-flight edit | `soma-store/tests/store.rs` (kills a writer process) |
| A8 JSON export round-trips byte-identically | `soma-core`, `soma-cli` tests |

## Layout

| Crate | Role |
|---|---|
| `soma-core` | Domain types, invariants I1–I5, invertible ops, overlays, JSON snapshot. No I/O |
| `soma-store` | SQLite (WAL, FTS5), migrations, persisted undo/redo, store worker thread |
| `soma-pdf` | PDFium behind a `PdfBackend` trait: tiles, per-char text geometry, outline, links, anchor capture/resolution |
| `soma-layout` | Barnes–Hut force layout (friction becomes rest length), Sugiyama layered, radial |
| `soma-ui` | egui app: reader, capture, graph strip, canvas, palettes |
| `soma-cli` | Headless init / add-doc / export / import / verify / search / roots / bench / demo-pdf |

## Getting started

```sh
./scripts/fetch-pdfium.sh                       # prebuilt libpdfium → vendor/pdfium
cargo run --release -p soma-ui -- paper.pdf     # workspace: ~/Documents/Soma/workspace.soma
cargo run --release -p soma-ui -- --workspace thesis.soma a.pdf b.pdf
cargo run --release -p soma-cli -- demo-pdf demo.pdf   # a sample stochastic-calculus PDF
```

Press **F1** in the app to see the keymap. The core loop:

1. Select text, then **Ctrl-1**. This highlights it and creates a node filed in system 1.
2. Select more text, **Ctrl-1**, then **L**. This links the previous node to the new one (`prerequisite-of`).
3. **Tab** opens the canvas. Focus a relation (click it, or **e**), then **N** to annotate it and **s** to file it. It now renders as a lozenge. Press **l** on another node to attach a relation to it.

## Tests

```sh
cargo test --workspace                            # PDFium tests skip if the library is absent
cargo run --release -p soma-cli -- bench --store  # A6 data side, persist/load timings
SOMA_AUTOTEST=/tmp/soma-at cargo run --release -p soma-ui -- --workspace /tmp/at.soma demo.pdf
```

The UI self-test drives the real capture → link → reify → delete/undo paths, saves screenshots and writes `report.txt`. It needs a visible display, because egui does not render while the screen is asleep or locked.

## Notes

- All PDFium calls are serialized behind a process-wide lock. PDFium crashes when used from several threads at once, even on different documents.
- Plain `h` highlights create no node. They are stored as anchors on a hidden per-document carrier entity, which graph views filter out.
- Unfiled nodes go to an implicit **Inbox** system (PRD open question 2). Hotkeyed systems give highlights their colour; everything else is neutral grey (open question 1).
