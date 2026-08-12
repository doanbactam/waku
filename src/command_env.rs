use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Build a command with the executable search path a terminal-launched Waku
/// normally inherits. Apps opened through LaunchServices only receive the
/// system PATH, which is not enough for script-based CLIs whose shebang uses
/// `/usr/bin/env` (for example, an npm-installed Codex launcher needs `node`).
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    if let Ok(path) = std::env::join_paths(executable_search_paths()) {
        command.env("PATH", path);
    }
    command
}

pub fn find_executable(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 {
        return executable_candidates(candidate)
            .into_iter()
            .find(|candidate| candidate.is_file());
    }
    executable_search_paths()
        .into_iter()
        .flat_map(|directory| executable_candidates(&directory.join(name)))
        .find(|candidate| candidate.is_file())
}

fn executable_candidates(path: &Path) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        executable_candidates_with_extensions(
            path,
            &std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()),
        )
    }
    #[cfg(not(windows))]
    {
        vec![path.to_path_buf()]
    }
}

#[cfg(windows)]
fn executable_candidates_with_extensions(path: &Path, path_ext: &str) -> Vec<PathBuf> {
    let candidates = vec![path.to_path_buf()];
    let extensions = path_ext
        .split(';')
        .filter(|extension| !extension.is_empty());
    if path.extension().is_some_and(|extension| {
        extensions.clone().any(|candidate| {
            candidate
                .trim_start_matches('.')
                .eq_ignore_ascii_case(&extension.to_string_lossy())
        })
    }) {
        candidates
    } else {
        candidates
            .into_iter()
            .chain(extensions.map(|extension| {
                let mut candidate = path.as_os_str().to_os_string();
                candidate.push(extension);
                PathBuf::from(candidate)
            }))
            .collect()
    }
}

/// Resolve a user-supplied binary override: `~` expands to the home
/// directory, a path must point at an existing file, and a bare name searches
/// the same directories as [`find_executable`].
pub fn resolve_binary_override(spec: &str) -> Option<PathBuf> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    if let Some(rest) = spec.strip_prefix("~/") {
        let candidate = dirs::home_dir()?.join(rest);
        return executable_candidates(&candidate)
            .into_iter()
            .find(|candidate| candidate.is_file());
    }
    find_executable(spec)
}

pub fn executable_search_path() -> Option<std::ffi::OsString> {
    std::env::join_paths(executable_search_paths()).ok()
}

fn executable_search_paths() -> Vec<PathBuf> {
    search_paths_from(
        std::env::var_os("PATH").as_deref(),
        dirs::home_dir().as_deref(),
    )
}

fn search_paths_from(path: Option<&OsStr>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut directories = path
        .map(|path| std::env::split_paths(path).collect::<Vec<_>>())
        .unwrap_or_default();
    if let Some(home) = home {
        directories.extend([
            home.join(".local/bin"),
            home.join(".bun/bin"),
            home.join(".cargo/bin"),
            home.join(".local/share/mise/shims"),
            home.join(".volta/bin"),
        ]);
    }
    #[cfg(unix)]
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ]);

    let mut seen = HashSet::new();
    directories.retain(|directory| seen.insert(directory.clone()));
    directories
}

#[cfg(windows)]
pub fn terminate_process_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn launch_services_path_is_extended_for_script_based_clis() {
        let home = Path::new("/Users/example");
        let paths = search_paths_from(Some(OsStr::new("/usr/bin:/bin")), Some(home));

        assert_eq!(paths[0], PathBuf::from("/usr/bin"));
        assert_eq!(paths[1], PathBuf::from("/bin"));
        assert!(paths.contains(&home.join(".bun/bin")));
        assert!(paths.contains(&home.join(".local/share/mise/shims")));
        assert!(paths.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert_eq!(
            paths
                .iter()
                .filter(|path| *path == Path::new("/bin"))
                .count(),
            1
        );
    }

    #[cfg(windows)]
    #[test]
    fn executable_candidates_follow_pathext() {
        let candidates =
            executable_candidates_with_extensions(Path::new("tool"), ".COM;.EXE;.BAT;.CMD");
        assert!(candidates.contains(&PathBuf::from("tool.EXE")));
        assert_eq!(
            executable_candidates_with_extensions(Path::new("tool.eXe"), ".COM;.EXE;.BAT;.CMD"),
            [PathBuf::from("tool.eXe")]
        );
    }
}
