mod schema_codegen;

fn main() {
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let generated = schema_codegen::generate(&root.join("src/lib.rs"))
        .expect("could not generate configuration metadata");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=schema_codegen.rs");
    for source in generated.sources {
        println!("cargo:rerun-if-changed={}", source.display());
    }
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(output.join("config_schema.rs"), generated.code).unwrap();
}
