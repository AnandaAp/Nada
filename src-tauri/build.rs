use std::path::Path;

fn read_jnn_key(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(value) = line.strip_prefix("NADA_JNN_KEY=") {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn main() {
    println!("cargo:rerun-if-env-changed=NADA_JNN_KEY");

    let env_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.env");
    println!("cargo:rerun-if-changed={}", env_path.display());

    let key = std::env::var("NADA_JNN_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| read_jnn_key(&env_path));

    if key.is_none() && std::env::var("PROFILE").as_deref() == Ok("release") {
        panic!("NADA_JNN_KEY is required for release builds");
    }

    if let Some(key) = key {
        println!("cargo:rustc-env=NADA_JNN_KEY={key}");
    }

    tauri_build::build()
}
