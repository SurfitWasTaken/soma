# Soma

A local-first, native Rust PDF reader whose annotations are a typed graph. See [PRD.md](PRD.md).

## Layout

| Crate | Role |
|---|---|
| `soma-core` | Domain types, invariants, graph ops — no I/O |
| `soma-store` | SQLite persistence, migrations, undo log, FTS |
| `soma-pdf` | PDFium wrapper, text extraction, anchor capture/resolution |
| `soma-layout` | Force, layered (Sugiyama) and radial layouts |
| `soma-ui` | egui app: reader, capture, graph canvas |
| `soma-cli` | Headless export / verify / bench |

## Getting started

```sh
./scripts/fetch-pdfium.sh          # downloads libpdfium into vendor/pdfium
cargo run --release -p soma-ui -- path/to/paper.pdf
```
