//! Parser for Tesseract's TSV output (OCR-TESS-CLI).

use sheaf_core::{OcrResult, Word};

/// Most words kept from one page; a page of noise must not exhaust memory.
const MAX_WORDS: usize = 50_000;

/// Columns: level page block par line word left top width height conf text.
/// Level 5 rows are words; everything else is layout structure.
pub fn parse(tsv: &str) -> OcrResult {
    let mut words = Vec::new();
    let mut line_no = 0u32;
    let mut last_line_key = None;
    for row in tsv.lines().skip(1) {
        let cols: Vec<&str> = row.splitn(12, '\t').collect();
        if cols.len() < 12 || cols[0] != "5" {
            continue;
        }
        let text = cols[11].trim();
        let num = |i: usize| cols[i].parse::<u32>().unwrap_or(0);
        let conf = cols[10].parse::<f32>().unwrap_or(-1.0);
        if text.is_empty() || conf < 0.0 {
            continue;
        }
        let key = (num(2), num(3), num(4));
        if last_line_key.is_some_and(|k| k != key) {
            line_no += 1;
        }
        last_line_key = Some(key);
        words.push(Word {
            text: text.to_string(),
            bbox: [num(6), num(7), num(8), num(9)],
            conf: conf.min(100.0),
            line: line_no,
        });
        if words.len() >= MAX_WORDS {
            break;
        }
    }
    let mean_conf = if words.is_empty() {
        0.0
    } else {
        words.iter().map(|w| w.conf).sum::<f32>() / words.len() as f32
    };
    OcrResult {
        words,
        mean_conf,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str =
        "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
1\t1\t0\t0\t0\t0\t0\t0\t2480\t3508\t-1\t\n\
4\t1\t1\t1\t1\t0\t300\t400\t900\t60\t-1\t\n\
5\t1\t1\t1\t1\t1\t300\t400\t250\t60\t96.5\tShartnoma\n\
5\t1\t1\t1\t1\t2\t580\t400\t90\t60\t91.0\t№\n\
5\t1\t1\t1\t2\t1\t300\t480\t200\t60\t40.25\tқўғҳ\n\
5\t1\t1\t1\t2\t2\t520\t480\t10\t60\t95\t \n\
5\t1\t2\t1\t1\t1\t300\t900\t120\t60\t88\ta\tb\n\
garbage line\n";

    #[test]
    fn words_lines_and_confidence() {
        let r = parse(SAMPLE);
        let texts: Vec<&str> = r.words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["Shartnoma", "№", "қўғҳ", "a\tb"]);
        assert_eq!(r.words[0].bbox, [300, 400, 250, 60]);
        assert_eq!(r.words.iter().map(|w| w.line).collect::<Vec<_>>(), [0, 0, 1, 2]);
        assert!((r.mean_conf - (96.5 + 91.0 + 40.25 + 88.0) / 4.0).abs() < 1e-4);
        assert_eq!(r.text(), "Shartnoma №\nқўғҳ\na\tb");
    }

    #[test]
    fn empty_and_malformed_input() {
        assert!(parse("").words.is_empty());
        assert_eq!(parse("header only\n").mean_conf, 0.0);
        assert!(parse("level\n5\t1\t1\n\x00\x01\x02").words.is_empty());
    }
}
