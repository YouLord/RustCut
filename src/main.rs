use std::path::PathBuf;

fn main() {
    let root = std::env::var_os("RUSTCUT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("current directory"));
    let bind = std::env::var("RUSTCUT_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
    if let Err(error) = rustcut::server::run(root, &bind) {
        eprintln!("RustCut could not start: {error}");
        std::process::exit(1);
    }
}
