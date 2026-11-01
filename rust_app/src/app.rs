use std::{path::PathBuf, sync::mpsc, thread, time::Duration};
use eframe::egui::{self, FontData, FontDefinitions, FontFamily, RichText, ThemePreference};
use melody_inference::{Environment, Event, Task, TranscriptionRequest};
use melody_player::{PlaybackState, Player, Song, Timeline};
use crate::{library, theme};

#[derive(PartialEq)]
enum Page { Transcribe, Play, Settings, About }

pub struct Workbench {
    root: PathBuf, page: Page, theme: ThemePreference, audio: Option<PathBuf>,
    vocal: bool, instrument: bool, name: String, task: Option<Task>,
    stage: String, progress: Option<f32>, logs: Vec<String>, started: Option<std::time::Instant>,
    files: Vec<PathBuf>, search: String, selected: Option<PathBuf>, song: Option<Song>,
    track: Option<usize>, accomp: bool, timeline: Option<Timeline>, player: Option<Player>,
    speed: f64, message: String, delete: Option<PathBuf>, playing_path: Option<PathBuf>, last_f8: bool, last_f12: bool,
    record_enabled: bool, recorder: Option<melody_recorder::Recorder>, recording_stopping: bool,
    worker_tx: mpsc::Sender<Work>, worker_rx: mpsc::Receiver<Work>, generation: u64,
    smoke_started: Option<std::time::Instant>,
    environment: Environment,
    last_output: Option<PathBuf>,
    artwork: egui::TextureHandle, icon: egui::TextureHandle,
}
enum Work { Library(Result<Vec<PathBuf>, String>), Imported(Result<PathBuf, String>),
    Audio(Option<PathBuf>), Compiled(u64, Result<(Song, Timeline), String>), Deleted(PathBuf, Result<(), String>) }

impl Workbench {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = FontDefinitions::default();
        for path in [r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\simhei.ttf"] {
            if let Ok(bytes) = std::fs::read(path) {
                fonts.font_data.insert("chinese".into(), FontData::from_owned(bytes).into());
                fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "chinese".into());
                fonts.families.entry(FontFamily::Monospace).or_default().push("chinese".into());
                break;
            }
        }
        cc.egui_ctx.set_fonts(fonts);
        theme::setup(&cc.egui_ctx);
        let theme = match cc.storage.and_then(|s| s.get_string("theme")).as_deref() {
            Some("light") => ThemePreference::Light, Some("dark") => ThemePreference::Dark, _ => ThemePreference::System,
        };
        cc.egui_ctx.set_theme(theme);
        let load_image = |name: &str, bytes: &[u8]| {
            let image = image::load_from_memory(bytes).expect("valid embedded image").to_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            cc.egui_ctx.load_texture(name, egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()), egui::TextureOptions::LINEAR)
        };
        let artwork = load_image("background", include_bytes!("../assets/background.png"));
        let icon = load_image("icon", include_bytes!("../assets/icon.png"));
        let (tx, rx) = mpsc::channel();
        let root = library::installation_root();
        let environment = Environment::inspect(&root);
        let s = Self { artwork, icon, root, environment, page: Page::Transcribe, theme,
            audio: None, vocal: true, instrument: true, name: String::new(), task: None,
            stage: "选择音频后开始扒谱".into(), progress: None, logs: Vec::new(), started: None,
            files: Vec::new(), search: String::new(), selected: None, song: None, track: None,
            accomp: true, timeline: None, player: None, speed: 1.0, message: "请选择曲目".into(),
            delete: None, playing_path: None, last_f8: false, last_f12: false, record_enabled: false, recorder: None, recording_stopping: false,
            worker_tx: tx, worker_rx: rx, generation: 0,
            smoke_started: std::env::args().any(|a| a == "--smoke").then(std::time::Instant::now), last_output: None };
        s.refresh(&cc.egui_ctx); s
    }
    fn refresh(&self, ctx: &egui::Context) {
        let (root, tx, ctx) = (self.root.clone(), self.worker_tx.clone(), ctx.clone());
        thread::spawn(move || { let _ = tx.send(Work::Library(library::scan(&root))); ctx.request_repaint(); });
    }
    fn compile(&mut self, ctx: &egui::Context) {
        let Some(path) = self.selected.clone() else { return };
        self.generation += 1; let generation = self.generation;
        self.timeline = None; self.message = "正在准备编排…".into();
        let (track, accomp, tx, ctx) = (self.track, self.accomp, self.worker_tx.clone(), ctx.clone());
        thread::spawn(move || { let result = melody_player::load_song(&path).and_then(|song| melody_player::compile(&song, track, accomp).map(|tl| (song, tl))); let _ = tx.send(Work::Compiled(generation, result)); ctx.request_repaint(); });
    }
    fn choose(&mut self, path: PathBuf, ctx: &egui::Context) { self.selected = Some(path); self.track = None; self.song = None; self.compile(ctx); }
    fn start_play(&mut self) {
        if let Some(player) = &self.player {
            match player.status().state {
                PlaybackState::Playing => { player.pause(); return; }
                PlaybackState::Paused => { if let Err(e) = player.resume() { self.message = e; } return; }
                _ => {}
            }
        }
        if self.recorder.is_some() { self.message = "正在保存上一次录音，请稍候。".into(); return; }
        match self.timeline.clone() {
            Some(t) => match Player::start(t, self.speed) { Ok(p) => {
                if self.record_enabled {
                    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
                    self.recorder = Some(melody_recorder::Recorder::start(self.root.join("player").join("录音").join(format!("演奏_{time}.wav"))));
                    self.recording_stopping = false;
                }
                self.playing_path = self.selected.clone(); self.player = Some(p);
            }, Err(e) => self.message = e },
            None => self.message = "请先选择 MIDI 并等待编排完成".into(),
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(item) = self.worker_rx.try_recv() {
            match item {
                Work::Library(Ok(files)) => self.files = files,
                Work::Library(Err(e)) | Work::Imported(Err(e)) => self.message = e,
                Work::Imported(Ok(path)) => { self.refresh(ctx); self.choose(path, ctx); }
                Work::Audio(path) => { if path.is_some() { self.audio = path; } }
                Work::Compiled(g, result) if g == self.generation => match result {
                    Ok((song, tl)) => { self.message = format!("音轨：{}  移调：{:+}  八度调整：{}  遗漏：{}  同键冲突：{}  伴奏音：{}", tl.track_name, tl.octave_shift, tl.folded_notes, tl.dropped, tl.conflicts, tl.accompaniment_notes); self.song = Some(song); self.timeline = Some(tl); }
                    Err(e) => self.message = format!("编排失败：{e}"),
                },
                Work::Compiled(..) => {},
                Work::Deleted(path, result) => match result {
                    Ok(()) => { if self.selected.as_ref() == Some(&path) { self.generation += 1; self.selected = None; self.song = None; self.timeline = None; self.message = "请选择曲目".into(); } self.refresh(ctx); }
                    Err(e) => self.message = format!("删除失败：{e}"),
                },
            }
        }
        if let Some(task) = &self.task {
            let events: Vec<_> = task.events.try_iter().collect();
            let mut done = false;
            for event in events { match event {
                Event::Log(s) => { self.logs.push(s); if self.logs.len() > 2000 { self.logs.remove(0); } }
                Event::Progress { stage, fraction } => { self.stage = stage; self.progress = fraction; }
                Event::Completed { output } => { self.stage = "任务完成".into(); self.progress = Some(1.0); done = true; if let Some(p) = output { self.last_output = Some(p.clone()); self.refresh(ctx); self.choose(p, ctx); } }
                Event::Failed(e) => { self.stage = format!("任务失败：{e}"); done = true; }
                Event::Cancelled => { self.stage = "任务已取消".into(); done = true; }
            }}
            if done { self.task = None; self.environment = Environment::inspect(&self.root); }
        }
        if let (Some(p), Some(r)) = (&self.player, &self.recorder) {
            match p.status().state {
                PlaybackState::Playing if !self.recording_stopping => r.resume(),
                PlaybackState::Paused if !self.recording_stopping => r.pause(),
                PlaybackState::Finished if !self.recording_stopping => { r.stop(true); self.recording_stopping = true; }
                PlaybackState::Stopped | PlaybackState::Error(_) if !self.recording_stopping => { r.stop(false); self.recording_stopping = true; }
                _ => {}
            }
        }
        if let Some(recorder) = &self.recorder {
            let events: Vec<_> = recorder.events.try_iter().collect(); let mut done = false;
            for event in events { match event {
                melody_recorder::Event::Started { sample_rate, channels } => self.logs.push(format!("录音已开始：{sample_rate} Hz / {channels} 声道")),
                melody_recorder::Event::Finished { path } => { self.message = path.map(|p| format!("录音已保存：{}", p.display())).unwrap_or_else(|| "录音已取消".into()); done = true; }
                melody_recorder::Event::Failed(e) => { self.message = format!("录音失败：{e}"); done = true; }
            }}
            if done { self.recorder = None; self.recording_stopping = false; }
        }
        let f8 = melody_player::hotkey_down(0x77); let f12 = melody_player::hotkey_down(0x7b);
        if f8 && !self.last_f8 && self.delete.is_none() { self.start_play(); }
        if f12 && !self.last_f12 { if let Some(p) = &self.player { p.stop(); } }
        self.last_f8 = f8; self.last_f12 = f12;
        ctx.request_repaint_after(Duration::from_millis(50));
    }
    fn choose_audio(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.worker_tx.clone(), ctx.clone());
        thread::spawn(move || { let p = rfd::FileDialog::new().add_filter("音频", &["mp3", "wav", "flac", "m4a", "ogg"]).pick_file(); let _ = tx.send(Work::Audio(p)); ctx.request_repaint(); });
    }
    fn import(&self, ctx: &egui::Context) {
        let (tx, ctx, root) = (self.worker_tx.clone(), ctx.clone(), self.root.clone());
        thread::spawn(move || { if let Some(p) = rfd::FileDialog::new().add_filter("MIDI", &["mid", "midi"]).pick_file() { let _ = tx.send(Work::Imported(library::import(&p, &root))); ctx.request_repaint(); } });
    }
    fn transcribe(&mut self) {
        let Some(audio) = self.audio.clone() else { self.stage = "请先选择音频".into(); return };
        let stems = match (self.vocal, self.instrument) { (true, true) => "both", (true, false) => "vocal", (false, true) => "inst", _ => { self.stage = "请至少选择一个声部".into(); return } };
        let req = TranscriptionRequest { root: self.root.clone(), audio, output_dir: self.root.join("out"), final_name: (!self.name.trim().is_empty()).then(|| self.name.trim().to_owned()), stems: stems.into() };
        match melody_inference::start_transcription(req) { Ok(t) => { self.task = Some(t); self.started = Some(std::time::Instant::now()); self.stage = "正在启动 CPU 模型…".into(); self.logs.clear(); self.progress = None; self.last_output = None; }, Err(e) => self.stage = e }
    }
}

impl eframe::App for Workbench {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("theme", match self.theme { ThemePreference::Light => "light", ThemePreference::Dark => "dark", ThemePreference::System => "system" }.into());
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone(); self.poll(&ctx);
        if self.smoke_started.is_some_and(|t| t.elapsed() >= Duration::from_secs(2)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let background = theme::palette(ui.visuals().dark_mode).background;
        egui::Panel::left("navigation").resizable(false).default_size(210.0)
            .frame(egui::Frame::new().fill(background).inner_margin(16)).show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let p = theme::palette(ui.visuals().dark_mode);
                ui.add(egui::Image::new((self.icon.id(), egui::vec2(38.0, 38.0))).corner_radius(8));
                ui.vertical(|ui| { ui.label(RichText::new("明音凛的").strong().size(16.0)); ui.label(RichText::new("旋律工坊").size(14.0).color(p.muted)); });
            });
            ui.add_space(30.0);
            for (page, title) in [(Page::Transcribe, "AI 智能扒谱"), (Page::Play, "三排按键演奏"), (Page::Settings, "环境与设置"), (Page::About, "关于")] {
                let selected = self.page == page;
                let p = theme::palette(ui.visuals().dark_mode);
                let button = egui::Button::selectable(selected, title).fill(if selected {p.soft} else {p.background}).stroke(egui::Stroke::NONE).corner_radius(7);
                if ui.add_sized([ui.available_width(), 42.0], button).clicked() { self.page = page; }
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.label(RichText::new("AI 推理  ·  CPU").color(theme::palette(ui.visuals().dark_mode).muted).size(13.0));
                ui.separator();
            });
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(background).inner_margin(egui::Margin::symmetric(26, 24))).show(ui, |ui| {
            let rect = ui.max_rect();
            let image_aspect = self.artwork.size()[0] as f32 / self.artwork.size()[1] as f32;
            let view_aspect = rect.width() / rect.height().max(1.0);
            let uv = if view_aspect < image_aspect { let width = view_aspect / image_aspect; egui::Rect::from_min_max(egui::pos2(1.0 - width, 0.0), egui::pos2(1.0, 1.0)) } else { let height = image_aspect / view_aspect; egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, height)) };
            ui.painter().image(self.artwork.id(), rect, uv, egui::Color32::WHITE);
            let overlay = if ui.visuals().dark_mode { egui::Color32::from_black_alpha(195) } else { egui::Color32::from_white_alpha(80) };
            ui.painter().rect_filled(rect, 0.0, overlay);
            egui::ScrollArea::vertical().id_salt("page_scroll").show(ui, |ui| match self.page {
                Page::Transcribe => { let width = ui.available_width(); if width > 550.0 { ui.set_max_width((width * 0.72).max(430.0)); } self.transcribe_ui(ui, &ctx); },
                Page::Play => self.play_ui(ui, &ctx),
                Page::Settings => self.settings_ui(ui, &ctx),
                Page::About => self.about_ui(ui),
            });
        });
        if let Some(path) = self.delete.clone() {
            let modal = egui::Modal::new(egui::Id::new("delete_midi")).show(&ctx, |ui| {
                ui.heading("删除 MIDI 文件");
                ui.label("将此文件移到回收站？"); ui.label(path.display().to_string());
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() { self.delete = None; }
                    if ui.button("移到回收站").clicked() {
                        self.delete = None;
                        if self.playing_path.as_ref() == Some(&path) && self.player.as_ref().is_some_and(|p| matches!(p.status().state, PlaybackState::Playing | PlaybackState::Paused)) {
                            self.message = "请先停止演奏，再删除这首曲目。".into();
                        } else {
                            let (p, tx, ctx) = (path.clone(), self.worker_tx.clone(), ctx.clone());
                            thread::spawn(move || { let result = trash::delete(&p).map_err(|e| e.to_string()); let _ = tx.send(Work::Deleted(p, result)); ctx.request_repaint(); });
                        }
                    }
                });
            });
            if modal.should_close() { self.delete = None; }
        }
    }
}

impl Drop for Workbench {
    fn drop(&mut self) { if let Some(task) = &self.task { task.cancel(); } if let Some(player) = &self.player { player.stop(); } if let Some(recorder) = &self.recorder { recorder.stop(false); } }
}

impl Workbench {
    fn transcribe_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.set_max_width(920.0);
        ui.heading("AI 智能扒谱");
        ui.label(RichText::new("选择一首音频，用 CPU 生成适合演奏的 MIDI。").color(theme::palette(ui.visuals().dark_mode).muted));
        ui.add_space(18.0);
        theme::card(ui, |ui| {
            ui.label(RichText::new("新建扒谱任务").size(18.0).strong());
            ui.label(RichText::new("选择音频和要保留的声部").color(theme::palette(ui.visuals().dark_mode).muted));
            ui.add_space(12.0);
            ui.label(RichText::new("原始音频").strong());
            ui.horizontal(|ui| {
                let mut name = self.audio.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "尚未选择音频文件".into());
                let path = self.audio.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                let width = (ui.available_width() - 120.0).max(100.0);
                ui.add_sized([width, 38.0], egui::TextEdit::singleline(&mut name).interactive(false)).on_hover_text(path);
                if ui.button("选择音频…").clicked() { self.choose_audio(ctx); }
            });
            ui.add_space(6.0);
            ui.label(RichText::new("成品名称").strong());
            ui.add_sized([ui.available_width().min(540.0), 38.0], egui::TextEdit::singleline(&mut self.name).hint_text("留空时使用音频名称"));
            ui.add_space(6.0);
            ui.label(RichText::new("扒谱声部").strong());
            ui.horizontal_wrapped(|ui| { ui.checkbox(&mut self.vocal, "人声主旋律"); ui.checkbox(&mut self.instrument, "器乐旋律"); });
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(self.task.is_none(), theme::primary("开始扒谱", ui.visuals().dark_mode)).clicked() { self.transcribe(); }
                if ui.add_enabled(self.task.is_some(), egui::Button::new("取消任务")).clicked() { if let Some(t) = &self.task { t.cancel(); self.stage = "正在取消…".into(); } }
            });
        });
        ui.add_space(16.0);
        theme::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("任务状态").size(17.0).strong());
                if self.task.is_some() { ui.spinner(); }
                if self.task.is_some() { if let Some(t) = self.started { ui.label(RichText::new(format!("已用时 {} 秒", t.elapsed().as_secs())).color(theme::palette(ui.visuals().dark_mode).muted)); } }
            });
            ui.label(&self.stage);
            if let Some(p) = self.progress { ui.label(RichText::new("当前阶段进度").small()); ui.add(egui::ProgressBar::new(p.clamp(0.0,1.0)).show_percentage().desired_width(ui.available_width())); }
            if self.last_output.is_some() && self.task.is_none() {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("前往演奏台").clicked() { self.page = Page::Play; }
                    if ui.button("打开输出目录").clicked() { let path = self.root.join("out"); thread::spawn(move || { let _ = std::process::Command::new("explorer.exe").arg(path).spawn(); }); }
                });
            }
            ui.add_space(4.0);
            egui::CollapsingHeader::new("详细日志").id_salt("transcribe_log").show(ui, |ui| {
                if self.logs.is_empty() { ui.label(RichText::new("任务运行后将在这里显示日志。").color(theme::palette(ui.visuals().dark_mode).muted)); }
                for s in &self.logs { ui.label(s); }
            });
        });
    }
    fn play_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("三排按键演奏");
        ui.label(RichText::new("选择曲目，在游戏的三排音符界面中演奏。").color(theme::palette(ui.visuals().dark_mode).muted));
        ui.add_space(18.0);
        if ui.available_width() < 690.0 {
            self.library_ui(ui, ctx); ui.add_space(14.0); self.play_detail_ui(ui, ctx);
        } else {
            let width = (ui.available_width() * 0.38).clamp(260.0, 360.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(egui::vec2(width, 580.0), egui::Layout::top_down(egui::Align::Min), |ui| self.library_ui(ui, ctx));
                ui.add_space(6.0);
                ui.vertical(|ui| self.play_detail_ui(ui, ctx));
            });
        }
    }
    fn library_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        theme::card(ui, |ui| {
            ui.label(RichText::new("本地 MIDI 曲库").size(18.0).strong());
            ui.label(RichText::new(format!("{} 首曲目", self.files.len())).color(theme::palette(ui.visuals().dark_mode).muted).small());
            ui.horizontal_wrapped(|ui| { if ui.button("导入 MIDI…").clicked() { self.import(ctx); } if ui.button("刷新").clicked() { self.refresh(ctx); } });
            ui.add_sized([ui.available_width(), 38.0], egui::TextEdit::singleline(&mut self.search).hint_text("搜索曲名"));
            let mut chosen = None; let mut count = 0;
            let available_height = ui.available_height().clamp(180.0, 480.0);
            egui::ScrollArea::vertical().id_salt("library").max_height(available_height).show(ui, |ui| {
                for path in &self.files {
                    let file = path.file_name().unwrap_or_default().to_string_lossy();
                    if !file.to_lowercase().contains(&self.search.to_lowercase()) { continue; }
                    count += 1;
                    let label = if path.parent().is_some_and(|p| p != self.root.join("out") && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("模型对比_"))) {
                        format!("{} / {}", path.parent().unwrap().file_name().unwrap_or_default().to_string_lossy(), file)
                    } else { file.to_string() };
                    ui.push_id(path, |ui| {
                        let p = theme::palette(ui.visuals().dark_mode);
                        let selected = self.selected.as_ref() == Some(path);
                        let button = egui::Button::selectable(selected, label).truncate().fill(if selected {p.soft} else {p.surface}).stroke(egui::Stroke::NONE);
                        let response = ui.add_sized([ui.available_width(), 36.0], button).on_hover_text(path.display().to_string());
                        if response.clicked() { chosen = Some(path.clone()); }
                        response.context_menu(|ui| { if ui.button("删除文件…").clicked() { self.delete = Some(path.clone()); ui.close(); } });
                    });
                }
                if count == 0 { ui.add_space(12.0); ui.label(RichText::new(if self.files.is_empty() { "曲库为空，导入 MIDI 开始演奏。" } else { "没有找到匹配的曲目。" }).color(theme::palette(ui.visuals().dark_mode).muted)); }
            });
            if let Some(path) = chosen { self.choose(path, ctx); }
        });
    }
    fn play_detail_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        theme::card(ui, |ui| {
            ui.label(RichText::new("曲目与演奏").size(18.0).strong());
            ui.add_space(6.0);
            let selected_name = self.selected.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "从左侧选择一首曲目".into());
            let selected_path = self.selected.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
            ui.add(egui::Label::new(RichText::new(selected_name).strong()).truncate()).on_hover_text(selected_path);
            if let Some(path) = &self.playing_path {
                if self.player.as_ref().is_some_and(|p| matches!(p.status().state, PlaybackState::Playing | PlaybackState::Paused)) {
                    ui.label(RichText::new(format!("正在演奏：{}", path.file_name().unwrap_or_default().to_string_lossy())).color(theme::palette(ui.visuals().dark_mode).muted));
                }
            }
            ui.add_space(8.0); ui.label(RichText::new("主旋律音轨").strong());
            let old = self.track;
            egui::ComboBox::from_id_salt("melody_track").width(ui.available_width().min(360.0)).selected_text(self.track.and_then(|i| self.song.as_ref()?.tracks.iter().find(|t| t.index == i).map(|t| t.name.clone())).unwrap_or_else(|| "自动识别".into())).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.track, None, "自动识别");
                if let Some(song) = &self.song { for t in &song.tracks { if !t.notes.is_empty() { ui.selectable_value(&mut self.track, Some(t.index), format!("{}（{} 音）", t.name, t.notes.len())); } } }
            });
            let changed = ui.checkbox(&mut self.accomp, "加入简化伴奏").changed();
            if old != self.track || changed { self.compile(ctx); }
            ui.add_space(8.0);
            let p = theme::palette(ui.visuals().dark_mode);
            egui::Frame::new().fill(p.soft).corner_radius(7).inner_margin(12).show(ui, |ui| { ui.set_width(ui.available_width()); ui.label("切换到游戏三排音符界面后按 F8 开始。"); ui.label("若游戏以管理员身份运行，请右键软件并选择以管理员身份运行。权限不一致可能导致无法演奏。"); ui.label(RichText::new(&self.message).small()); });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| { if ui.add(theme::primary("开始 / 暂停（F8）", ui.visuals().dark_mode)).clicked() { self.start_play(); } if ui.button("停止（F12）").clicked() { if let Some(p) = &self.player { p.stop(); } } });
            ui.scope(|ui| {
                let line = theme::palette(ui.visuals().dark_mode).line;
                ui.visuals_mut().widgets.inactive.bg_fill = line;
                ui.add(egui::Slider::new(&mut self.speed, 0.5..=2.0).text("播放速度"));
            });
            ui.checkbox(&mut self.record_enabled, "录制系统声音（WAV）");
            if let Some(path) = self.selected.clone() { if ui.button("删除选中曲目…").clicked() { self.delete = Some(path); } }
            if let Some(p) = &self.player { let status = p.status();
                let text = match &status.state { PlaybackState::Playing => "演奏中".to_owned(), PlaybackState::Paused => "已暂停".into(), PlaybackState::Finished => "演奏完成".into(), PlaybackState::Stopped => "已停止".into(), PlaybackState::Error(e) => format!("演奏出错：{e}") };
                ui.label(format!("状态：{text}")); if status.duration > 0.0 { ui.add(egui::ProgressBar::new((status.position / status.duration) as f32).show_percentage()); } }
        });
    }
    fn about_ui(&self, ui: &mut egui::Ui) {
        ui.set_max_width(920.0);
        ui.heading("关于");
        ui.add_space(18.0);
        theme::card(ui, |ui| {
            ui.label(RichText::new("明音凛的旋律工坊").size(20.0).strong());
            ui.label("音频转 MIDI · 三排按键演奏");
            ui.add_space(12.0);
            ui.label("作者：xuasas");
            ui.horizontal_wrapped(|ui| {
                ui.label("项目地址：");
                ui.hyperlink_to("github.com/xuasas/nte-guitar-midi", "https://github.com/xuasas/nte-guitar-midi");
            });
            ui.label(RichText::new("项目尚未上传时，链接可能暂时无法访问。").small());
        });
    }
    fn settings_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.set_max_width(920.0);
        ui.heading("环境与设置");
        ui.label(RichText::new("管理模型环境与界面外观。").color(theme::palette(ui.visuals().dark_mode).muted));
        ui.add_space(18.0);
        theme::card(ui, |ui| {
            ui.label(RichText::new("模型环境").size(18.0).strong());
            ui.label(RichText::new("AI 扒谱使用本机 CPU 运行。").color(theme::palette(ui.visuals().dark_mode).muted));
            ui.add_space(8.0);
            if self.environment.ready() { ui.label("状态：模型文件已就绪"); } else { ui.label(format!("状态：缺少 {}", self.environment.missing.join("、"))); }
            ui.label(RichText::new(format!("安装目录：{}", self.root.display())).small().color(theme::palette(ui.visuals().dark_mode).muted));
            ui.horizontal_wrapped(|ui| {
                if ui.button("重新检查").clicked() { self.environment = Environment::inspect(&self.root); }
                if ui.add_enabled(self.task.is_none(), egui::Button::new("安装 / 修复模型环境")).clicked() {
                    match melody_inference::start_install(&self.root) { Ok(t) => { self.task = Some(t); self.stage = "正在修复环境".into(); self.started = Some(std::time::Instant::now()); }, Err(e) => self.stage = e }
                }
                if ui.add_enabled(self.task.is_some(), egui::Button::new("取消任务")).clicked() { if let Some(t) = &self.task { t.cancel(); } }
            });
            if self.task.is_some() || self.stage.starts_with("任务失败") { ui.label(&self.stage); }
            egui::CollapsingHeader::new("详细日志").id_salt("settings_log").show(ui, |ui| { for s in &self.logs { ui.label(s); } });
        });
        ui.add_space(16.0);
        theme::card(ui, |ui| {
            ui.label(RichText::new("界面外观").size(18.0).strong());
            ui.label(RichText::new("选择适合你的显示主题。").color(theme::palette(ui.visuals().dark_mode).muted));
            ui.horizontal_wrapped(|ui| { ui.selectable_value(&mut self.theme, ThemePreference::System, "跟随系统"); ui.selectable_value(&mut self.theme, ThemePreference::Light, "浅色"); ui.selectable_value(&mut self.theme, ThemePreference::Dark, "深色"); });
        });
        ctx.set_theme(self.theme);
    }
}
