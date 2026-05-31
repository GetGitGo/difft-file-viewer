use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::difft_probe::subprocess_command;

const CLANG_FORMAT: &str = "clang-format";
/// Deliberately not `.clang-format`, to avoid clashing with an existing project config.
const CLANG_FORMAT_CONFIG: &str = ".clangformat";
const CLANG_FORMAT_CACHE: &str = ".clangformat.cache";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct FileMtime {
    modified_secs: u64,
    modified_nanos: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FormatCache {
    config: Option<FileMtime>,
    entries: HashMap<String, FileMtime>,
}

/// If preconditions hold, run `clang-format -i` on C/C++ input files that exist.
/// Skips files when `.clangformat` and the input file mtimes match `.clangformat.cache`.
/// Returns a warning string when formatting was attempted but failed for some files.
pub fn preprocess_input_files(paths: &[PathBuf]) -> Option<String> {
    let config = non_empty_clang_format_in_cwd()?;
    if !clang_format_available() {
        return None;
    }

    let cwd = env::current_dir().ok()?;
    let config_mtime = match file_mtime(&config) {
        Ok(mtime) => mtime,
        Err(err) => return Some(err),
    };

    let mut cache = load_cache(&cwd);
    let mut cache_dirty = false;
    let mut warnings = Vec::new();

    if cache.config != Some(config_mtime) {
        cache.config = Some(config_mtime);
        cache.entries.clear();
        cache_dirty = true;
    }

    for path in paths {
        if !path.is_file() || !is_c_cpp_file(path) {
            continue;
        }

        let cache_key = match cache_key(path) {
            Ok(key) => key,
            Err(err) => {
                warnings.push(format!(
                    "clang-format cache skipped for {}: {err}",
                    path.display()
                ));
                continue;
            }
        };

        let current_mtime = match file_mtime(path) {
            Ok(mtime) => mtime,
            Err(err) => {
                warnings.push(format!(
                    "clang-format skipped for {}: {err}",
                    path.display()
                ));
                continue;
            }
        };

        if cache.entries.get(&cache_key) == Some(&current_mtime) {
            continue;
        }

        if let Err(err) = format_file_in_place(&config, path) {
            warnings.push(format!("clang-format failed for {}: {err}", path.display()));
            continue;
        }

        match file_mtime(path) {
            Ok(mtime) => {
                cache.entries.insert(cache_key, mtime);
                cache_dirty = true;
            }
            Err(err) => warnings.push(format!(
                "clang-format cache not updated for {}: {err}",
                path.display()
            )),
        }
    }

    if cache_dirty {
        if let Err(err) = save_cache(&cwd, &cache) {
            warnings.push(err);
        }
    }

    if warnings.is_empty() {
        None
    } else {
        Some(warnings.join("\n"))
    }
}

fn cache_key(path: &Path) -> Result<String, String> {
    path.canonicalize()
        .map(|p| p.display().to_string())
        .map_err(|e| format!("failed to canonicalize {}: {e}", path.display()))
}

fn file_mtime(path: &Path) -> Result<FileMtime, String> {
    let meta = fs::metadata(path).map_err(|e| format!("failed to stat {}: {e}", path.display()))?;
    let modified = meta
        .modified()
        .map_err(|e| format!("failed to read mtime for {}: {e}", path.display()))?;
    let duration = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("invalid mtime for {}: {e}", path.display()))?;
    Ok(FileMtime {
        modified_secs: duration.as_secs(),
        modified_nanos: duration.subsec_nanos(),
    })
}

fn load_cache(cwd: &Path) -> FormatCache {
    let path = cwd.join(CLANG_FORMAT_CACHE);
    fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_cache(cwd: &Path, cache: &FormatCache) -> Result<(), String> {
    let path = cwd.join(CLANG_FORMAT_CACHE);
    let content = serde_json::to_string_pretty(cache)
        .map_err(|e| format!("failed to encode {}: {e}", path.display()))?;
    fs::write(&path, content).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn non_empty_clang_format_in_cwd() -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    let config = cwd.join(CLANG_FORMAT_CONFIG);
    let meta = fs::metadata(&config).ok()?;
    if !meta.is_file() || meta.len() == 0 {
        return None;
    }
    Some(config)
}

fn clang_format_available() -> bool {
    subprocess_command(CLANG_FORMAT)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn format_file_in_place(config: &Path, file: &Path) -> Result<(), String> {
    let style = format!("-style=file:{}", config.display());
    let output = subprocess_command(CLANG_FORMAT)
        .arg(style)
        .arg("-i")
        .arg(file)
        .output()
        .map_err(|e| format!("failed to run {CLANG_FORMAT}: {e}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.trim().is_empty() {
        Err(format!("exit status {}", output.status))
    } else {
        Err(stderr.trim().to_owned())
    }
}

fn is_c_cpp_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "c" | "cc" | "cpp" | "cxx" | "c++" | "h" | "hh" | "hpp" | "hxx" | "h++"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_common_c_cpp_extensions() {
        assert!(is_c_cpp_file(Path::new("foo.cpp")));
        assert!(is_c_cpp_file(Path::new("bar.C")));
        assert!(is_c_cpp_file(Path::new("baz.hpp")));
        assert!(!is_c_cpp_file(Path::new("main.rs")));
        assert!(!is_c_cpp_file(Path::new("readme")));
    }

    #[test]
    fn format_cache_roundtrip_json() {
        let cache = FormatCache {
            config: Some(FileMtime {
                modified_secs: 1,
                modified_nanos: 2,
            }),
            entries: HashMap::from([(
                "/tmp/sample.cpp".into(),
                FileMtime {
                    modified_secs: 3,
                    modified_nanos: 4,
                },
            )]),
        };
        let json = serde_json::to_string(&cache).unwrap();
        let decoded: FormatCache = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.config, cache.config);
        assert_eq!(decoded.entries, cache.entries);
    }

    #[test]
    fn file_mtime_reads_metadata() {
        let file = env::temp_dir().join(format!("difft-clang-{}.cpp", std::process::id()));
        fs::write(&file, "int x;\n").unwrap();
        assert_eq!(file_mtime(&file).unwrap(), file_mtime(&file).unwrap());
        let _ = fs::remove_file(&file);
    }
}
