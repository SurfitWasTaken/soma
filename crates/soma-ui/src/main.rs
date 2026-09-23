//! Soma — a local-first PDF reader whose annotations are a typed graph.

mod app;
mod canvas;
mod reader;
mod render;
mod workspace;

use std::path::PathBuf;

fn main() -> eframe::Result {
    let mut workspace = None;
    let mut pdfs = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-w" | "--workspace" => workspace = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!("usage: soma-ui [--workspace FILE.soma] [PDF ...]");
                return Ok(());
            }
            _ => pdfs.push(PathBuf::from(a)),
        }
    }
    let workspace = workspace.unwrap_or_else(workspace::default_workspace_path);
    if let Err(e) = soma_pdf::pdfium() {
        eprintln!("{e}");
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Soma").with_inner_size([1440.0, 920.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Soma",
        options,
        Box::new(move |cc| {
            let app = app::SomaApp::new(cc, workspace, pdfs).map_err(|e| e.to_string())?;
            Ok(Box::new(app))
        }),
    )
}
