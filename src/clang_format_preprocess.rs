use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::difft_probe::subprocess_command;

const CLANG_FORMAT: &str = "clang-format";
/// Deliberately not `.clang-format`, to avoid clashing with an existing project config.
const CLANG_FORMAT_CONFIG: &str = ".clangformat";
const CLANG_FORMAT_CACHE: &str = ".clangformat.cache";
const CLANG_FORMAT_CACHE_DIR: &str = ".clangformat.cache.d";
/// Filename clang-format looks for when using `-style=file` (copied from [CLANG_FORMAT_CONFIG]).
const CLANG_FORMAT_CACHE_CONFIG: &str = ".clang-format";
/// Bump when clang-format invocation or cache layout changes (invalidates stale copies).
const FORMAT_CACHE_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct FileMtime {
    modified_secs: u64,
    modified_nanos: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FormatCache {
    #[serde(default)]
    version: u32,
    config: Option<FileMtime>,
    entries: HashMap<String, FileMtime>,
}

/// Same gate as diff pre-process: non-empty `./.clangformat` in cwd and `clang-format` on `PATH`.
pub fn formatting_enabled() -> bool {
    non_empty_clang_format_in_cwd().is_some() && clang_format_available()
}

/// Returns whether quit should format `file_c`.
pub fn should_format_file_c(path_c: &Path) -> bool {
    formatting_enabled() && is_c_cpp_file(path_c)
}

/// Write formatted `file_c` using `./.clangformat` (via cache-dir `.clang-format` copy).
pub fn spawn_detached_format_file_c(path_c: &Path, lines: &[String]) -> Result<(), String> {
    if !should_format_file_c(path_c) {
        return Ok(());
    }
    let config = non_empty_clang_format_in_cwd()
        .ok_or_else(|| "missing non-empty .clangformat in cwd".to_owned())?;
    let formatted = format_lines_in_memory(&config, path_c, lines)?;
    write_lines(path_c, &formatted)
}

/// Remove `./.clangformat.cache.d/` and `./.clangformat.cache` from cwd (best-effort).
pub fn cleanup_cache() {
    let Ok(cwd) = env::current_dir() else {
        return;
    };
    let _ = fs::remove_dir_all(cwd.join(CLANG_FORMAT_CACHE_DIR));
    let _ = fs::remove_file(cwd.join(CLANG_FORMAT_CACHE));
}

fn write_lines(path: &Path, lines: &[String]) -> Result<(), String> {
    let content = if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n")
    };
    fs::write(path, content).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// Returns paths to pass to `difft` for A and B.
///
/// When formatting is enabled, writes formatted **copies** under `./.clangformat.cache.d/`
/// and leaves the original files untouched.
pub fn diff_input_paths(path_a: &Path, path_b: &Path) -> (PathBuf, PathBuf, Option<String>) {
    let mut warnings = Vec::new();
    let a = prepare_diff_input(path_a, &mut warnings);
    let b = prepare_diff_input(path_b, &mut warnings);
    let note = if warnings.is_empty() {
        None
    } else {
        Some(warnings.join("\n"))
    };
    (a, b, note)
}

fn prepare_diff_input(path: &Path, warnings: &mut Vec<String>) -> PathBuf {
    let Some(config) = non_empty_clang_format_in_cwd() else {
        return path.to_path_buf();
    };
    if !clang_format_available() {
        return path.to_path_buf();
    }
    if !path.is_file() || !is_c_cpp_file(path) {
        return path.to_path_buf();
    }

    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            warnings.push(format!("clang-format skipped for {}: {err}", path.display()));
            return path.to_path_buf();
        }
    };

    let config_mtime = match file_mtime(&config) {
        Ok(mtime) => mtime,
        Err(err) => {
            warnings.push(err);
            return path.to_path_buf();
        }
    };

    let cache_key = match cache_key(path) {
        Ok(key) => key,
        Err(err) => {
            warnings.push(format!(
                "clang-format cache skipped for {}: {err}",
                path.display()
            ));
            return path.to_path_buf();
        }
    };

    let source_mtime = match file_mtime(path) {
        Ok(mtime) => mtime,
        Err(err) => {
            warnings.push(format!("clang-format skipped for {}: {err}", path.display()));
            return path.to_path_buf();
        }
    };

    let mut cache = load_cache(&cwd);
    let mut cache_dirty = false;

    if cache.config != Some(config_mtime) {
        cache.config = Some(config_mtime);
        cache.entries.clear();
        cache_dirty = true;
        let _ = fs::remove_dir_all(cwd.join(CLANG_FORMAT_CACHE_DIR));
    }

    let formatted_path = formatted_cache_path(&cwd, &cache_key, path);

    if cache.entries.get(&cache_key) == Some(&source_mtime) && formatted_path.is_file() {
        if cache_dirty {
            if let Err(err) = save_cache(&cwd, &cache) {
                warnings.push(err);
            }
        }
        return formatted_path;
    }

    if let Err(err) = format_file_to_cache(&config, path, &formatted_path) {
        warnings.push(format!("clang-format failed for {}: {err}", path.display()));
        return path.to_path_buf();
    }

    cache.entries.insert(cache_key, source_mtime);
    cache_dirty = true;

    if cache_dirty {
        if let Err(err) = save_cache(&cwd, &cache) {
            warnings.push(err);
        }
    }

    formatted_path
}

fn formatted_cache_path(cwd: &Path, cache_key: &str, source: &Path) -> PathBuf {
    let ext = source
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("txt");
    cwd.join(CLANG_FORMAT_CACHE_DIR)
        .join(format!("{cache_key}.{ext}"))
}

fn cache_key(path: &Path) -> Result<String, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize {}: {e}", path.display()))?;
    let mut hasher = DefaultHasher::new();
    canonical.display().to_string().hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
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
    let cache: FormatCache = fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default();
    if cache.version != FORMAT_CACHE_VERSION {
        let _ = fs::remove_dir_all(cwd.join(CLANG_FORMAT_CACHE_DIR));
        return FormatCache {
            version: FORMAT_CACHE_VERSION,
            ..Default::default()
        };
    }
    cache
}

fn save_cache(cwd: &Path, cache: &FormatCache) -> Result<(), String> {
    let path = cwd.join(CLANG_FORMAT_CACHE);
    let cache = FormatCache {
        version: FORMAT_CACHE_VERSION,
        ..cache.clone()
    };
    let content = serde_json::to_string_pretty(&cache)
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

fn sync_cache_config(cache_dir: &Path, config: &Path) -> Result<(), String> {
    fs::create_dir_all(cache_dir)
        .map_err(|e| format!("failed to create {}: {e}", cache_dir.display()))?;
    fs::copy(config, cache_dir.join(CLANG_FORMAT_CACHE_CONFIG))
        .map_err(|e| format!("failed to copy {}: {e}", config.display()))?;
    Ok(())
}

fn format_lines_in_memory(
    config: &Path,
    path_hint: &Path,
    lines: &[String],
) -> Result<Vec<String>, String> {
    let cwd = env::current_dir().map_err(|e| format!("clang-format skipped: {e}"))?;
    let ext = path_hint
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("txt");
    let cache_dir = cwd.join(CLANG_FORMAT_CACHE_DIR);
    fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("failed to create {}: {e}", cache_dir.display()))?;
    let temp_in = cache_dir.join(format!("_file_c_in_{}.{ext}", std::process::id()));
    let temp_out = cache_dir.join(format!("_file_c_out_{}.{ext}", std::process::id()));

    let content = if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n")
    };
    fs::write(&temp_in, content).map_err(|e| format!("failed to write {}: {e}", temp_in.display()))?;

    format_file_to_cache(config, &temp_in, &temp_out)?;

    let formatted = fs::read_to_string(&temp_out)
        .map_err(|e| format!("failed to read {}: {e}", temp_out.display()))?;
    let _ = fs::remove_file(&temp_in);
    let _ = fs::remove_file(&temp_out);

    Ok(if formatted.is_empty() {
        vec![]
    } else {
        formatted.lines().map(str::to_owned).collect()
    })
}

fn format_file_to_cache(config: &Path, file: &Path, out: &Path) -> Result<(), String> {
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }

    let cache_dir = out
        .parent()
        .ok_or_else(|| "clang-format cache dir missing".to_owned())?;
    sync_cache_config(cache_dir, config)?;

    let content = fs::read_to_string(file)
        .map_err(|e| format!("failed to read {}: {e}", file.display()))?;
    let ext = file
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("txt");
    let assume_filename = cache_dir.join(format!("_fmt.{ext}"));

    // clang-format only recognizes `.clang-format`; `-style=file:.clangformat` fails on Windows.
    // Copy viewer config into the cache dir and point `-assume-filename` there so lookup works
    // for sources on any path (including network drives).
    let mut cmd = subprocess_command(CLANG_FORMAT);
    cmd.arg("-style=file")
        .arg("-assume-filename")
        .arg(&assume_filename)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run {CLANG_FORMAT}: {e}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "clang-format stdin unavailable".to_owned())?
        .write_all(content.as_bytes())
        .map_err(|e| format!("failed to write clang-format stdin: {e}"))?;

    let output = child
        .wait_with_output()
        .map_err(|e| format!("failed to wait for {CLANG_FORMAT}: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return if stderr.trim().is_empty() {
            Err(format!("exit status {}", output.status))
        } else {
            Err(stderr.trim().to_owned())
        };
    }

    fs::write(out, &output.stdout)
        .map_err(|e| format!("failed to write {}: {e}", out.display()))
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
            version: FORMAT_CACHE_VERSION,
            config: Some(FileMtime {
                modified_secs: 1,
                modified_nanos: 2,
            }),
            entries: HashMap::from([(
                "abc123".into(),
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

    #[test]
    fn cache_key_is_stable_for_same_path() {
        let file = env::temp_dir().join(format!("difft-clang-key-{}.cpp", std::process::id()));
        fs::write(&file, "int x;\n").unwrap();
        let a = cache_key(&file).unwrap();
        let b = cache_key(&file).unwrap();
        assert_eq!(a, b);
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn should_format_file_c_skips_non_cpp() {
        assert!(!should_format_file_c(Path::new("foo.rs")));
    }

    #[test]
    fn spawn_format_skips_non_cpp() {
        assert!(
            spawn_detached_format_file_c(Path::new("foo.rs"), &["fn main() {}".into()]).is_ok()
        );
    }
}
