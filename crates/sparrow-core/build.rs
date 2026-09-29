use std::{fs, path::Path};

fn hash_sources(path: &Path, hash: &mut blake3::Hasher) {
    let mut entries = fs::read_dir(path)
        .expect("core sources are available")
        .map(|entry| entry.expect("source entry is readable").path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        if entry.is_dir() {
            hash_sources(&entry, hash);
        } else if entry.extension().is_some_and(|extension| extension == "rs") {
            println!("cargo:rerun-if-changed={}", entry.display());
            hash.update(entry.to_string_lossy().as_bytes());
            hash.update(&fs::read(entry).expect("core source is readable"));
        }
    }
}

fn main() {
    let mut hash = blake3::Hasher::new();
    hash_sources(Path::new("src"), &mut hash);
    for path in ["build.rs", "Cargo.toml", "../../Cargo.lock"] {
        println!("cargo:rerun-if-changed={path}");
        hash.update(&fs::read(path).expect("build input is readable"));
    }
    println!(
        "cargo:rustc-env=SPARROW_CATALOG_BUILD={}",
        hash.finalize().to_hex()
    );
}
