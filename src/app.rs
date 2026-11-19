//! App shell: wires library + reader screens, owns the store and scan worker.

use std::sync::{mpsc::Receiver, Arc, Mutex};

use crate::library::Library;
use crate::reader::Reader;
use crate::scan::{self, ScanMsg};
use crate::store::{SharedStore, Store};

#[derive(Clone)]
pub enum ScanStatus {
    Idle,
    Scanning {
        done: usize,
        total: usize,
        current: String,
    },
    Thumbs,
    Done(Vec<String>),
}

#[derive(PartialEq)]
enum Mode {
    Library,
    Reader,
}

pub struct MarginalApp {
    store: SharedStore,
    mode: Mode,
    library: Library,
    reader: Option<Reader>,
    scan_rx: Option<Receiver<ScanMsg>>,
    status: ScanStatus,
    errors_pending: Vec<String>,
    // frame-time probe (MARGINAL_FRAME_TRACE=1): rolling avg + worst, so
    // perf questions get answered on a stopwatch, not by vibes
    frame_acc_us: u64,
    frame_cnt: u32,
    frame_worst_us: u64,
}

impl MarginalApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let store = Store::open_default()?;
        let last = store.get_meta("last_folder")?;
        let store: SharedStore = Arc::new(Mutex::new(store));

        let mut library = Library::new(store.clone());
        if let Some(f) = last.clone() {
            library.set_folder(f);
        }

        cc.egui_ctx.set_visuals({
            let mut v = egui::Visuals::dark();
            crate::theme::apply_visuals(&mut v);
            v
        });
        crate::theme::register_fonts(&cc.egui_ctx);

        // test/automation hook: MARGINAL_OPEN=<book_id>:<page> (screenshot workflow)
        let mut mode = Mode::Library;
        let mut reader = None;
        if let Ok(spec) = std::env::var("MARGINAL_OPEN") {
            let spec = spec.trim().to_string();
            if let Some((id_s, page_s)) = spec.split_once(':') {
                if let (Ok(id), Ok(page)) = (id_s.parse::<i64>(), page_s.parse::<u32>()) {
                    if let Ok(book) = crate::store::with_store(&store, |s| s.book_by_id(id)) {
                        let mut r = Reader::open(store.clone(), book);
                        r.go_to_page(page);
                        reader = Some(r);
                        mode = Mode::Reader;
                    }
                }
            }
        }

        Ok(MarginalApp {
            store,
            mode,
            library,
            reader,
            scan_rx: None,
            status: ScanStatus::Idle,
            errors_pending: Vec::new(),
            frame_acc_us: 0,
            frame_cnt: 0,
            frame_worst_us: 0,
        })
    }

    fn start_scan(&mut self, folder: String) {
        self.status = ScanStatus::Scanning {
            done: 0,
            total: 0,
            current: String::new(),
        };
        self.scan_rx = Some(scan::spawn_scan(self.store.clone(), folder.into()));
    }

    fn poll_scan(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.scan_rx else { return };
        let mut repaint = false;
        let mut done_now = None;
        while let Ok(msg) = rx.try_recv() {
            repaint = true;
            match msg {
                ScanMsg::Progress { done, total, current } => {
                    self.status = ScanStatus::Scanning { done, total, current };
                }
                ScanMsg::ScanningThumbs | ScanMsg::Thumb { .. } => {
                    self.status = ScanStatus::Thumbs;
                }
                ScanMsg::Done { errors } => {
                    done_now = Some(errors);
                }
            }
        }
        if let Some(errors) = done_now {
            self.errors_pending.extend(errors);
            self.status = ScanStatus::Done(std::mem::take(&mut self.errors_pending));
            self.scan_rx = None;
        }
        if repaint {
            ctx.request_repaint();
        }
    }
}

impl eframe::App for MarginalApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_scan(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // frame-time probe: measure THIS pass's actual work (not the idle
        // gap since the last pass!); ~90 passes per print. Every ~90 frames
        // print the rolling average and the worst pass, plus ppp (the
        // page-quality ladder keys off it)
        let probe_start = std::time::Instant::now();
        self.poll_scan(ui.ctx());

        match self.mode {
            Mode::Library => {
                let status = self.status.clone();
                self.library.show(ui, &status);

                if let Some(f) = self.library.folder_request.take() {
                    crate::store::with_store(&self.store, |s| s.set_meta("last_folder", &f)).ok();
                    self.library.set_folder(f.clone());
                    self.start_scan(f);
                }
                if let Some(f) = self.library.rescan_request.take() {
                    self.start_scan(f);
                }
                if let Some(id) = self.library.open_request.take() {
                    match crate::store::with_store(&self.store, |s| s.book_by_id(id)) {
                        Ok(book) => {
                            self.reader = Some(Reader::open(self.store.clone(), book));
                            self.mode = Mode::Reader;
                            ui.ctx().request_repaint();
                        }
                        Err(e) => self.library.error = Some(format!("opening failed: {e}")),
                    }
                }
            }
            Mode::Reader => {
                if !self.reader.is_some() {
                    self.mode = Mode::Library;
                    return;
                }

                let mut back = false;
                let jump: Option<(i64, u32)> = {
                    let Some(reader) = self.reader.as_mut() else {
                        self.mode = Mode::Library;
                        return;
                    };
                    reader.show(ui, &mut || back = true);
                    reader.pending_jump.take()
                };

                // cross-book navigation from links
                if let Some((new_id, page)) = jump {
                    if let Ok(book) =
                        crate::store::with_store(&self.store, |s| s.book_by_id(new_id))
                    {
                        let mut nr = Reader::open(self.store.clone(), book);
                        nr.go_to_page(page);
                        self.reader = Some(nr);
                    }
                }

                if back {
                    self.reader = None;
                    self.mode = Mode::Library;
                }
            }
        }

        // probe bookkeeping (see the start of this fn)
        if std::env::var("MARGINAL_FRAME_TRACE").as_deref() == Ok("1") {
            let us = probe_start.elapsed().as_micros() as u64;
            self.frame_acc_us += us;
            self.frame_cnt += 1;
            self.frame_worst_us = self.frame_worst_us.max(us);
            if self.frame_cnt >= 60 {
                eprintln!(
                    "[frame-trace] {} avg={:.2}ms worst={:.2}ms ppp={:.3}",
                    if cfg!(debug_assertions) { "debug" } else { "release" },
                    self.frame_acc_us as f64 / self.frame_cnt as f64 / 1000.0,
                    self.frame_worst_us as f64 / 1000.0,
                    ui.ctx().pixels_per_point()
                );
                self.frame_acc_us = 0;
                self.frame_cnt = 0;
                self.frame_worst_us = 0;
            }
        }
    }
}
