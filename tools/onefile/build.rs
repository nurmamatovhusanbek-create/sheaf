//! Packs the pinned third-party engines and the licence file into the executable (SEC-SUPPLY).
//!
//! Every file listed in `vendor/SHA256SUMS` that belongs to the target platform is hashed against
//! its pin, compressed and written to `$OUT_DIR/payload.bin`; the hashes (and the licence file's)
//! go to `$OUT_DIR/payload.sums`. A missing or altered file fails the build. The executable then
//! carries both the files and the hashes it checks them against at run time.

use flate2::{write::ZlibEncoder, Compression};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn add(payload: &mut Vec<u8>, sums: &mut String, path: &str, bytes: &[u8], hash: &str) {
    let mut packed = ZlibEncoder::new(Vec::new(), Compression::best());
    packed.write_all(bytes).expect("compress in memory");
    let packed = packed.finish().expect("compress in memory");
    payload.extend((path.len() as u32).to_le_bytes());
    payload.extend(path.as_bytes());
    payload.extend((bytes.len() as u64).to_le_bytes());
    payload.extend((packed.len() as u64).to_le_bytes());
    payload.extend(packed);
    let _ = writeln!(sums, "{hash}  {path}");
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let root = manifest.join("../..");
    let vendor = root.join("vendor");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let platform = if windows { "win-x64" } else { "linux-x64" };

    let pins_path = vendor.join("SHA256SUMS");
    println!("cargo:rerun-if-changed={}", pins_path.display());
    let pins = std::fs::read_to_string(&pins_path).expect("vendor/SHA256SUMS is readable");

    let (mut payload, mut sums) = (Vec::new(), String::new());
    for line in pins.lines() {
        let Some((want, path)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let path = path.trim_start().trim_start_matches('*');
        // `dl/` holds build inputs (archives, the Tesseract installer), not engines.
        let other_platform = ["win-x64", "linux-x64"]
            .iter()
            .any(|p| *p != platform && path.contains(&format!("/{p}/")));
        if path.starts_with("dl/") || other_platform {
            continue;
        }
        let file = vendor.join(path);
        println!("cargo:rerun-if-changed={}", file.display());
        let bytes = std::fs::read(&file).unwrap_or_else(|_| {
            panic!(
                "vendor/{path} is missing: run `bash tools/fetch-vendor` (for a Windows build from Linux, \
                 unpack the Tesseract files too: see vendor/extract.tsv)"
            )
        });
        let got = hex_sha256(&bytes);
        assert!(
            got.eq_ignore_ascii_case(want),
            "vendor/{path} does not match its pin in SHA256SUMS"
        );
        add(&mut payload, &mut sums, path, &bytes, &got);
    }
    assert!(!sums.is_empty(), "no engines found to embed");

    // The licence texts of everything bundled; the About window shows them.
    let licences = root.join("THIRD_PARTY_LICENSES");
    println!("cargo:rerun-if-changed={}", licences.display());
    if let Ok(bytes) = std::fs::read(&licences) {
        add(
            &mut payload,
            &mut sums,
            "THIRD_PARTY_LICENSES",
            &bytes,
            &hex_sha256(&bytes),
        );
    }
    let _ = Path::new(&out);
    std::fs::write(out.join("payload.bin"), payload).expect("write payload");
    std::fs::write(out.join("payload.sums"), sums).expect("write payload hashes");
}
