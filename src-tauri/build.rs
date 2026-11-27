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

    #[cfg(windows)]
    {
        let icon_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/icon.ico");
        println!("cargo:rerun-if-changed={}", icon_path.display());
        if icon_path.exists() {
            let mut res = winres::WindowsResource::new();
            res.set_icon(icon_path.to_str().unwrap_or("icons/icon.ico"));
            // Locate Windows SDK rc.exe if not already on PATH so the PE icon resource is embedded
            let sdk_bin_root = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
            if let Ok(entries) = std::fs::read_dir(sdk_bin_root) {
                let mut versions: Vec<_> = entries
                    .flatten()
                    .map(|e| e.path().join("x64"))
                    .filter(|p| p.join("rc.exe").exists())
                    .collect();
                versions.sort();
                if let Some(latest_x64) = versions.last() {
                    if let Some(s) = latest_x64.to_str() {
                        res.set_toolkit_path(s);
                    }
                }
            }
            if let Err(e) = res.compile() {
                println!("cargo:warning=winres icon embedding failed: {e}");
            }
        }
    }
}
