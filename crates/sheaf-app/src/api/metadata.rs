//! CMD-TOOL-RUN for TOOL-METADATA. Reading is a plain call (it takes a moment: call it off
//! the UI thread); writing a changed copy is an app job like any other.

use super::*;
use sheaf_core::proto::{MetaEdit, MetadataInfo};

impl Backend {
    /// What a PDF says about itself.
    pub fn metadata_read(&self, file: &Path) -> Result<MetadataInfo> {
        self.ready()?;
        crate::metadata::read(&self.shared, file, &NoCtl)
    }

    /// Write a changed copy next to the original (`NAME.clean.pdf` or `NAME.edited.pdf`).
    pub fn metadata_edit(self: &Arc<Self>, file: PathBuf, change: MetaEdit) -> Result<u64> {
        self.ready()?;
        if let MetaEdit::Set(fields) = &change {
            fields.validate()?;
        }
        Ok(self.spawn_job("metadata", move |ctx| {
            let (out, info) = crate::metadata::edit(&ctx.shared, &file, &change, None, ctx)?;
            ctx.add_output(&out);
            Ok(JobOutput {
                outputs: vec![out],
                detail: json!({
                    "count": 1,
                    "pages": info.pages,
                    "stripped": matches!(change, MetaEdit::Strip),
                }),
            })
        }))
    }
}
