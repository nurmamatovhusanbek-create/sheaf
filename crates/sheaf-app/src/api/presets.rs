//! CMD-PRESETS: list, get, save, delete and run (PRESET-FORMAT, FLOW-PRESET).
//! Presets live in the config folder beside `settings.json`. A run works on
//! files, not on the open document: it starts its own scratch session so the
//! workspace in the window is never touched, and it is an app job like any
//! other (progress, cancel, Reveal).

use super::*;
use sheaf_core::Preset;

/// Most files one preset run takes.
pub const MAX_PRESET_FILES: usize = 200;

impl Backend {
    fn presets_dir(&self) -> Result<&Path> {
        self.config_dir
            .as_deref()
            .ok_or_else(|| AppError::new(ErrorCode::Unsupported, "presets need a config folder"))
    }

    /// CMD-PRESETS list: the names that can be used, sorted.
    pub fn presets_list(&self) -> Vec<String> {
        self.presets_dir().map(sheaf_store::presets::list).unwrap_or_default()
    }

    pub fn preset_get(&self, name: &str) -> Result<Preset> {
        sheaf_store::presets::load(self.presets_dir()?, name)
    }

    /// Save (or replace); returns the preset as stored.
    pub fn preset_save(&self, preset: Preset) -> Result<Preset> {
        sheaf_store::presets::save(self.presets_dir()?, &preset)
    }

    pub fn preset_delete(&self, name: &str) -> Result<()> {
        let dir = self.presets_dir()?;
        sheaf_store::presets::delete(dir, name)?;
        // A deleted preset cannot stay the default.
        let mut settings = lock(&self.settings);
        if settings.default_preset.as_deref() == Some(name) {
            settings.default_preset = None;
            let _ = settings.save(dir);
        }
        Ok(())
    }

    /// CMD-PRESETS run on `files` → the job id.
    pub fn preset_run(self: &Arc<Self>, name: &str, files: Vec<PathBuf>) -> Result<u64> {
        self.ready()?;
        if files.is_empty() || files.len() > MAX_PRESET_FILES {
            return Err(AppError::invalid("a preset run takes 1 to 200 files"));
        }
        let preset = self.preset_get(name)?;
        let me = Arc::clone(self);
        Ok(self.spawn_job("preset", move |ctx| {
            let workers = lock(&me.settings).worker_count();
            let mut app = App::start(&me.engines, &me.base_dir, workers)?;
            app.shared().set_allow_network(lock(&me.settings).allow_network_paths);
            let result = crate::preset::run_preset(
                &mut app,
                &preset,
                &files,
                None,
                &|fraction, _| ctx.progress(fraction),
                &|| ctx.cancelled() || me.closing.is_set(),
            );
            let _ = app.close();
            let report = result?;
            for out in &report.outputs {
                ctx.add_output(out);
            }
            if report.outputs.is_empty() {
                // Nothing came out: the first reason is the job's error.
                return Err(report
                    .failures
                    .into_iter()
                    .next()
                    .map_or_else(|| AppError::internal("nothing was produced"), |(_, e)| e));
            }
            Ok(JobOutput {
                outputs: report.outputs.clone(),
                detail: json!({
                    "count": report.outputs.len(),
                    "failed": report.failures.len(),
                    "inputBytes": report.bytes_before,
                    "outputBytes": report.bytes_after,
                }),
            })
        }))
    }
}
