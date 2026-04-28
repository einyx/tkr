use std::path::Path;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let filters_dir = Path::new(&manifest).join("../../filters");
    println!("cargo:rustc-env=TKR_BUNDLED_FILTERS_DIR={}", filters_dir.display());
    println!("cargo:rerun-if-changed=../../filters");
}
