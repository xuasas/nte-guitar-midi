#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod library;
mod theme;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1150.0, 760.0])
            .with_min_inner_size([720.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "明音凛的旋律工坊",
        options,
        Box::new(|cc| Ok(Box::new(app::Workbench::new(cc)))),
    )
}
