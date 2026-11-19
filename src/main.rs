//! main.rs: parse a couple of CLI conveniences, then boot the GUI.

mod app;
mod library;
mod links;
mod model;
mod pdf;
mod reader;
mod scan;
mod sidecar;
mod store;
mod theme;

use anyhow::Result;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Headless conveniences for testing without a display:
    //   marginal --scan <dir>         index a folder and print the library
    //   marginal --render <pdf> <page> <out.png>
    if args.len() == 2 && args[0] == "--scan" {
        match headless_scan(&args[1]) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("scan failed: {e:#}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() == 4 && args[0] == "--render" {
        match headless_render(&args[1], args[2].parse().unwrap_or(1), &args[3]) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("render failed: {e:#}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("marginal — marginalia & post-it annotations for your book folder");
        println!();
        println!("Usage:");
        println!("  marginal                     run the graphical app");
        println!("  marginal --scan <dir>        headless: index a folder, print results");
        println!("  marginal --render <pdf> <page> <out.png>  headless render one page");
        return;
    }

    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([940.0, 600.0])
            .with_icon(load_icon()),
        // the old AMD card stumbles under the wgpu/Vulkan stack (desktop-wide
        // compositor stalls caught on the cpu watch): speak the older,
        // calmer OpenGL instead. Swap back by deleting this line.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    let result = eframe::run_native(
        "Marginal — marginalia for your library",
        native,
        Box::new(|cc| Ok(Box::new(app::MarginalApp::new(cc)?))),
    );
    if let Err(e) = result {
        eprintln!("eframe error: {e}");
        std::process::exit(1);
    }
}

fn load_icon() -> egui::IconData {
    // simple 32x32 rounded sticky-note-ish icon drawn in code
    let (w, h) = (32usize, 32usize);
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let rx = (x as f32 - 16.0).abs();
            let ry = (y as f32 - 16.0).abs();
            let inside = rx.max(ry) < 13.0;
            let fold = x > 20 && y > 20 && (x + y) > 44;
            if inside && !fold {
                (px[i], px[i + 1], px[i + 2], px[i + 3]) = (254, 233, 129, 255);
            } else {
                (px[i], px[i + 1], px[i + 2], px[i + 3]) = (0, 0, 0, 0);
            }
        }
    }
    egui::IconData {
        width: w as u32,
        height: h as u32,
        rgba: px,
    }
}

// ---------------------------------------------------------------------------
// headless helpers
// ---------------------------------------------------------------------------

fn headless_scan(dir: &str) -> Result<()> {
    let store = std::sync::Arc::new(std::sync::Mutex::new(store::Store::open_default()?));
    crate::store::with_store(&store, |s| s.set_meta("last_folder", dir))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let store2 = store.clone();
    let dir_owned = dir.to_string();
    let handle = std::thread::spawn(move || {
        scan::scan_store(&store2, std::path::Path::new(&dir_owned), &tx)
    });
    for msg in rx {
        match msg {
            scan::ScanMsg::Progress { done, total, current } => {
                println!("  [{done}/{total}] {current}");
            }
            scan::ScanMsg::ScanningThumbs => println!("  rendering thumbnails…"),
            scan::ScanMsg::Thumb {
                book_id,
                page_count,
                ..
            } => println!("  thumb ok for book #{book_id} ({page_count} pages)"),            scan::ScanMsg::Done { errors } => {
                for e in &errors {
                    eprintln!("  error: {e}");
                }
            }
        }
    }
    handle.join().ok();
    let books = crate::store::with_store(&store, |s| s.books())?;
    println!("\nLibrary ({} books):", books.len());
    for b in &books {
        println!(
            "  #{:<4} {:<20} {:<6} {:>3} pages  {}",
            b.id,
            truncate(&b.title, 20),
            b.format.label(),
            b.page_count.map(|n| n.to_string()).unwrap_or("-".into()),
            b.path
        );
    }
    Ok(())
}

fn headless_render(pdf_path: &str, page: u32, out_png: &str) -> Result<()> {
    let doc = pdf::OpenDoc::open(std::path::Path::new(pdf_path))?;
    println!("{} pages", doc.page_count());
    let img = doc.render(page.saturating_sub(1), 1000)?;
    let png = pdf::rgba_to_png(&img)?;
    std::fs::write(out_png, png)?;
    println!("wrote {} ({}x{})", out_png, img.width, img.height);
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}
