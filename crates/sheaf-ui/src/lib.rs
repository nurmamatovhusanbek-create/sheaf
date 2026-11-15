//! MOD-UI: native egui views (ADR-NATIVE-UI): the main window and the action wheel.

mod aim;
mod app;
mod appicon;
mod dialogs;
mod icons;
mod jobs;
mod meta_ui;
mod platform;
mod presets_ui;
mod preview;
mod renderer;
mod search;
mod side;
mod textures;
mod theme;
mod tray;
mod wheel;

pub mod format;
pub mod geometry;
pub mod i18n;

pub use app::{run_window, SheafApp, UiConfig};
pub use appicon::rgba as app_icon_rgba;
pub use renderer::{opengl_unusable, RENDERER_ENV};
pub use wheel::{run_wheel, take_file_list, WheelApp, WheelConfig, WheelJob};

/// What the window and wheel entry points return.
pub type UiResult = eframe::Result;
