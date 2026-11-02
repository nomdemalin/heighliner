use std::sync::Arc;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

mod app;
mod engine;
mod osx;
mod ui;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,heighliner=debug")),
        )
        .with_target(false)
        .init();

    let paths = engine::paths::AppPaths::resolve()?;
    paths.ensure_dirs()?;
    tracing::info!(?paths, "resolved application paths");

    let config = engine::config::Config::load_or_default(&paths.config_file)?;
    tracing::info!(
        watches = config.watches.len(),
        rules = config.rules.len(),
        "config loaded"
    );
    let index = engine::index::Index::open(&paths.index_file)?;

    let engine_handle = engine::Engine::spawn(engine::EngineInit {
        paths: paths.clone(),
        config: config.clone(),
        index: Arc::new(index),
    });

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Heighliner")
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Heighliner",
        native_options,
        Box::new(move |cc| Ok(Box::new(app::HeighlinerApp::new(cc, engine_handle)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe exited: {e}"))?;

    Ok(())
}
