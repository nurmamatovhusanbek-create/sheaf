//! Words for the job bar (UI-JOBBAR).

use crate::format::{bytes, percent_change};
use crate::i18n::I18n;
use sheaf_app::jobs::JobView;

/// Words for a job's kind (`tool:compress` → "Compress").
pub fn kind_label(t: &I18n, kind: &str) -> String {
    match kind.strip_prefix("tool:") {
        Some(tool) => t.t(&format!("tool.{tool}.name")),
        None => t.t(&format!("job.{kind}")),
    }
}

/// A scan has no total, so the count of pages kept so far is its progress. Pages
/// that arrived before a cancel or a failure are kept (SCAN-FAILURES), and say so.
fn scan_summary(t: &I18n, job: &JobView, pages: i64) -> String {
    let count = t.tn("job.pages", pages, &[]);
    let kept = || t.tv("job.pages_kept", &[("pages", &count)]);
    match job.state {
        "failed" => {
            let why = t.t(job.error.as_ref().map_or("error.internal", |e| e.key));
            if pages > 0 {
                format!("{why} {}", kept())
            } else {
                why
            }
        }
        "cancelled" if pages > 0 => format!("{} · {}", t.t("job.cancelled"), kept()),
        "cancelled" => t.t("job.cancelled"),
        "queued" => t.t("job.queued"),
        _ => count,
    }
}

/// The one-line state or result of a job.
pub fn summary(t: &I18n, job: &JobView) -> String {
    let d = &job.detail;
    let num = |k: &str| d.get(k).and_then(|v| v.as_u64());
    if job.kind == "scan" {
        return scan_summary(t, job, num("pages").unwrap_or(0) as i64);
    }
    match job.state {
        "failed" => t.t(job.error.as_ref().map_or("error.internal", |e| e.key)),
        "cancelled" => t.t("job.cancelled"),
        "queued" => t.t("job.queued"),
        "done" => {
            if job.kind == "ocr" {
                let (a, b, c) = (
                    num("done").unwrap_or(0),
                    num("skipped").unwrap_or(0),
                    num("failed").unwrap_or(0),
                );
                return t.tv(
                    "job.ocr_result",
                    &[
                        ("done", &a.to_string()),
                        ("skipped", &b.to_string()),
                        ("failed", &c.to_string()),
                    ],
                );
            }
            if let Some(files) = d.get("files").and_then(|f| f.as_array()) {
                let (before, out) = (num("inputBytes").unwrap_or(0), num("outputBytes").unwrap_or(0));
                let sizes = if out > 0 && before > 0 {
                    format!(
                        " · {} → {} ({}%)",
                        bytes(before),
                        bytes(out),
                        percent_change(before, out)
                    )
                } else if out > 0 {
                    format!(" · {}", bytes(out))
                } else {
                    String::new()
                };
                let enc = files
                    .iter()
                    .find_map(|f| f.get("encryption").and_then(|e| e.as_str()).map(|e| format!(" · {e}")))
                    .unwrap_or_default();
                return format!("{}{sizes}{enc}", t.tn("job.files", files.len() as i64, &[]));
            }
            if job.kind == "metadata" {
                return t.t("job.metadata_done");
            }
            if job.kind == "repair" {
                // Say how much came back, not only that a file was written.
                let files = t.tn("job.files", num("count").unwrap_or(0) as i64, &[]);
                let pages = t.tn("job.pages", num("pages").unwrap_or(0) as i64, &[]);
                let failed = num("failed")
                    .filter(|n| *n > 0)
                    .map(|n| format!(" · {}", t.tn("job.failed", n as i64, &[])))
                    .unwrap_or_default();
                return format!("{files} · {pages}{failed}");
            }
            if let Some(count) = num("count") {
                let skipped = num("skipped")
                    .filter(|s| *s > 0)
                    .map(|s| format!(" · {}", t.tn("job.skipped", s as i64, &[])))
                    .unwrap_or_default();
                // Files a preset could not process (the others are kept).
                let failed = num("failed")
                    .filter(|n| *n > 0)
                    .map(|n| format!(" · {}", t.tn("job.failed", n as i64, &[])))
                    .unwrap_or_default();
                return format!("{}{skipped}{failed}", t.tn("job.files", count as i64, &[]));
            }
            t.t("job.done")
        }
        _ => format!("{}%", (job.progress * 100.0).round() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(kind: &str, state: &'static str, detail: serde_json::Value) -> JobView {
        JobView {
            id: 1,
            kind: kind.into(),
            state,
            progress: 0.5,
            outputs: vec![],
            error: None,
            detail,
        }
    }

    #[test]
    fn results_read_like_sentences() {
        let en = I18n::new("en");
        assert_eq!(kind_label(&en, "tool:compress"), "Compress");
        assert_eq!(kind_label(&en, "export"), "Export");
        assert_eq!(summary(&en, &job("export", "running", json!(null))), "50%");
        let compress =
            json!({"files": [{"encryption": null}], "inputBytes": 4_200_000u64, "outputBytes": 1_300_000u64});
        assert_eq!(
            summary(&en, &job("tool:compress", "done", compress)),
            "1 file · 4.0 MB → 1.2 MB (-69%)"
        );
        let ocr = json!({"done": 3, "skipped": 1, "failed": 0});
        assert_eq!(
            summary(&en, &job("ocr", "done", ocr)),
            "3 recognized, 1 already had text, 0 failed"
        );
        assert_eq!(
            summary(&I18n::new("ru"), &job("export", "cancelled", json!(null))),
            "Отменено"
        );
        let locked = json!({"files": [{"encryption": "AES-256"}], "inputBytes": 0, "outputBytes": 900});
        assert!(summary(&en, &job("tool:protect", "done", locked)).ends_with("AES-256"));

        // A scan shows the pages kept so far, also when it stopped early.
        let scan = |state: &'static str, pages: u64| job("scan", state, json!({ "pages": pages }));
        assert_eq!(summary(&en, &scan("running", 1)), "1 page");
        assert_eq!(summary(&en, &scan("done", 12)), "12 pages");
        assert_eq!(summary(&en, &scan("cancelled", 3)), "Cancelled · 3 pages kept");
        assert_eq!(summary(&en, &scan("cancelled", 0)), "Cancelled");
        let mut jam = scan("failed", 2);
        jam.error = Some(sheaf_app::api::ErrorDto::from(&sheaf_core::AppError::new(
            sheaf_core::ErrorCode::DeviceError,
            "paper jam",
        )));
        assert!(summary(&en, &jam).ends_with("2 pages kept"), "{}", summary(&en, &jam));
        assert_eq!(kind_label(&en, "scan"), "Scan");
        assert_eq!(kind_label(&en, "preset"), "Preset");
        assert_eq!(kind_label(&en, "repair"), "Repair");
        assert_eq!(
            summary(
                &en,
                &job("repair", "done", json!({"count": 2, "pages": 9, "failed": 1}))
            ),
            "2 files · 9 pages · 1 failed"
        );
        assert_eq!(
            summary(&en, &job("preset", "done", json!({"count": 3, "failed": 1}))),
            "3 files · 1 failed"
        );
    }
}
