use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `CREATE_NO_WINDOW` — avoid flashing a console when spawning a subprocess on Windows.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Build a subprocess command without spawning a console window on Windows.
pub fn subprocess_command<S: AsRef<OsStr>>(program: S) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new(program);
        cmd.creation_flags(CREATE_NO_WINDOW);
        return cmd;
    }
    #[cfg(not(windows))]
    Command::new(program)
}

/// Build a subprocess command for `difft` without spawning a console on Windows.
pub fn difft_command(path: &Path) -> Command {
    subprocess_command(path)
}

fn difft_binary_name() -> &'static str {
    if cfg!(windows) {
        "difft.exe"
    } else {
        "difft"
    }
}

fn difft_path_variants(path: PathBuf) -> Vec<PathBuf> {
    let mut paths = vec![path.clone()];
    if env::consts::EXE_SUFFIX.is_empty() || path.extension().is_some() {
        return paths;
    }

    paths.push(PathBuf::from(format!(
        "{}{}",
        path.display(),
        env::consts::EXE_SUFFIX
    )));
    paths
}

fn difft_works(path: &Path) -> bool {
    difft_command(path)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn first_working_path(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    for path in paths {
        if path.is_file() && difft_works(&path) {
            return Some(path);
        }
    }
    None
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(cwd) = env::current_dir() {
        paths.push(
            cwd.join("difftastic")
                .join("target")
                .join("debug")
                .join(difft_binary_name()),
        );
        paths.push(
            cwd.join("difftastic")
                .join("target")
                .join("release")
                .join(difft_binary_name()),
        );
    }

    if let Ok(path) = env::var("DIFT_PATH") {
        paths.extend(difft_path_variants(PathBuf::from(path)));
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join(difft_binary_name()));
        }
    }

    paths.push(PathBuf::from(difft_binary_name()));

    paths
}

fn path_lookup_hint() -> Option<PathBuf> {
    let lookup = subprocess_command(if cfg!(windows) { "where" } else { "which" })
        .arg("difft")
        .output()
        .ok()?;

    if !lookup.status.success() {
        return None;
    }

    String::from_utf8_lossy(&lookup.stdout)
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| PathBuf::from(line.trim()))
}

const VERIFY_STEP: &str = "\nVerification:\n  difft --version";

pub fn install_message() -> String {
    if cfg!(target_os = "macos") {
        format!(
            "Install difft:\n\
             \n\
              brew install difftastic{VERIFY_STEP}\n\
             \n\
             Or build the sibling clone in this repo:\n\
              cargo build --manifest-path difftastic/Cargo.toml\n\
              difft-file-viewer --difft difftastic/target/debug/difft file-a file-b\n\
             \n\
             Or set DIFT_PATH to the full path of an existing difft binary."
        )
    } else if cfg!(target_os = "windows") {
        format!(
            "Install difft (pick one):\n\
             \n\
              scoop install difftastic\n\
              winget install Wilfred.difftastic\n\
              choco install difftastic{VERIFY_STEP}\n\
             \n\
             Or build the sibling clone in this repo:\n\
              cargo build --manifest-path difftastic/Cargo.toml\n\
              difft-file-viewer --difft difftastic\\target\\debug\\difft.exe file-a file-b\n\
             \n\
             Or set DIFT_PATH to the full path of an existing difft.exe."
        )
    } else {
        format!(
            "Install difft (pick one that matches your system):\n\
             \n\
              sudo pacman -S difftastic        # Arch Linux\n\
              nix-env --install difftastic     # Nix\n\
              sudo dnf install difftastic      # Fedora\n\
              sudo pkg install difftastic      # FreeBSD{VERIFY_STEP}\n\
             \n\
             Or build the sibling clone in this repo:\n\
              cargo build --manifest-path difftastic/Cargo.toml\n\
              difft-file-viewer --difft difftastic/target/debug/difft file-a file-b\n\
             \n\
             Or set DIFT_PATH to the full path of an existing difft binary."
        )
    }
}

pub fn resolve_difft(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        if let Some(resolved) = first_working_path(difft_path_variants(path.clone())) {
            return Ok(resolved);
        }
        return Err(format!(
            "--difft does not point to a working difft binary: {}\n\n{}",
            path.display(),
            install_message()
        ));
    }
    probe_difft()
}

pub fn probe_difft() -> Result<PathBuf, String> {
    if let Ok(path) = env::var("DIFT_PATH") {
        let path = PathBuf::from(&path);
        if let Some(resolved) = first_working_path(difft_path_variants(path.clone())) {
            return Ok(resolved);
        }
        return Err(format!(
            "DIFT_PATH does not point to a working difft binary: {}\n\n{}",
            path.display(),
            install_message()
        ));
    }

    for path in candidate_paths() {
        if path.is_absolute() || path.components().count() > 1 {
            if let Some(path) = first_working_path(std::iter::once(path)) {
                return Ok(path);
            }
        } else if difft_works(&path) {
            return Ok(path);
        }
    }

    if let Some(path) = path_lookup_hint() {
        if difft_works(&path) {
            return Ok(path);
        }
    }

    Err(format!(
        "difft not found.\n\n{}",
        install_message()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn difft_path_variants_adds_exe_suffix_on_windows() {
        let variants = difft_path_variants(PathBuf::from("/tmp/difft"));
        if cfg!(windows) {
            assert_eq!(variants.len(), 2);
            assert_eq!(variants[1], PathBuf::from("/tmp/difft.exe"));
        } else {
            assert_eq!(variants.len(), 1);
        }
    }

    #[test]
    fn difft_binary_name_matches_platform() {
        if cfg!(windows) {
            assert_eq!(difft_binary_name(), "difft.exe");
        } else {
            assert_eq!(difft_binary_name(), "difft");
        }
    }
}
