//! PurrCode's native desktop IDE.
//!
//! This is a real application window, not a page served over loopback and not
//! an extension inside somebody else's editor: one Rust binary, drawing its own
//! interface, talking to the one PurrCode daemon over the authenticated local
//! API. `purrcode ide` opens it, and it never launches a browser.

pub mod app;
pub mod daemon;
pub mod icons;
pub mod markdown;
pub mod model;
pub mod terminal;
pub mod theme;
pub mod welcome;

use std::path::PathBuf;

use anyhow::{Context, Result};

pub use app::Config;

/// The window icon, derived from the supplied mascot (PRD §20).
const ICON: &[u8] = include_bytes!("../../../brand/icons/256.png");

/// Open the IDE and run until the user closes the window.
///
/// This blocks the calling thread. On macOS and Windows the platform requires
/// the event loop to own the main thread, so `purrcode ide` calls this before
/// entering any async runtime.
pub fn run(config: Config) -> Result<()> {
    let title = match config.repository.as_ref() {
        Some(repository) => format!(
            "PurrCode — {}",
            repository
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| repository.display().to_string())
        ),
        // No folder yet: the start screen is what opens.
        None => "PurrCode".to_owned(),
    };

    let viewport = egui::ViewportBuilder::default()
        .with_title(&title)
        .with_inner_size([1480.0, 940.0])
        .with_min_inner_size([900.0, 600.0])
        .with_app_id("dev.purrcode.PurrCode");
    let viewport = match load_icon() {
        Some(icon) => viewport.with_icon(icon),
        // A missing icon must not stop the product from opening.
        None => viewport,
    };
    // The window's own command bar runs edge to edge under the traffic lights
    // instead of below a second, system-drawn strip that repeats the title we
    // already show. The buttons stay — only the bar behind them goes away — so
    // close, minimise and zoom keep working exactly as the platform expects.
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "PurrCode",
        options,
        Box::new(move |context| {
            egui_extras::install_image_loaders(&context.egui_ctx);
            Ok(Box::new(app::PurrCodeIde::new(&context.egui_ctx, config)) as Box<dyn eframe::App>)
        }),
    )
    .map_err(|error| anyhow::anyhow!("PurrCode IDE could not open a window: {error}"))
}

fn load_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(ICON).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

/// Resolve the daemon token the same way every other PurrCode client does.
///
/// The token is read from the platform data directory, never from the
/// repository and never from an environment variable a child process could
/// inherit (PRD §17.1, §29).
pub fn default_token_path() -> Result<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        dirs_home()?
            .join("Library")
            .join("Application Support")
            .join("dev.PurrCode.PurrCode")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or(dirs_home()?)
            .join("PurrCode")
            .join("PurrCode")
            .join("data")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs_home().unwrap_or_default().join(".local").join("share"))
            .join("purrcode")
    };
    Ok(base.join("daemon.token"))
}

fn dirs_home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("no home directory is set")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_icon_is_a_real_image_derived_from_the_mascot() {
        let icon = load_icon().expect("the bundled brand icon must decode");
        assert_eq!(icon.width, 256);
        assert_eq!(icon.height, 256);
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        // The mascot sits on a transparent square, so the corner must not be
        // opaque — an opaque corner would mean we shipped the padded logo sheet.
        assert_eq!(icon.rgba[3], 0, "the icon corner should be transparent");
    }

    #[test]
    fn the_token_path_lives_outside_the_repository() {
        let path = default_token_path().expect("token path");
        assert!(path.ends_with("daemon.token"));
        assert!(
            !path.starts_with(std::env::current_dir().unwrap_or_default()),
            "the daemon token must never be resolved inside a user repository"
        );
    }
}
