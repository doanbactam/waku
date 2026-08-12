use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use std::ffi::CStr;
#[cfg(target_os = "macos")]
use std::mem::MaybeUninit;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStringExt;
#[cfg(target_os = "macos")]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;

const LOGIN_SHELL_PATH_TIMEOUT: Duration = Duration::from_secs(5);
const INTERACTIVE_SHELL_PATH_TIMEOUT: Duration = Duration::from_secs(3);
const SHELL_PATH_COMMAND: &str = "/usr/bin/printenv PATH > \"$WAKU_SHELL_PATH_CAPTURE_FILE\"";

static LOGIN_SHELL_PATH: OnceLock<RwLock<Option<OsString>>> = OnceLock::new();
static SHELL_PATH_CAPTURE_ID: AtomicU64 = AtomicU64::new(0);

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

/// Spawn `command` with `SIGCHLD` unblocked in the child. On macOS, libdispatch
/// worker threads (which back GPUI's background executor) block `SIGCHLD`, and
/// a process spawned from such a thread inherits the blocked mask. That breaks
/// provider-side async process reapers. The caller's mask is restored as soon
/// as the child has been created.
pub(crate) fn spawn(command: &mut Command) -> io::Result<Child> {
    with_sigchld_unblocked(|| command.spawn())
}

/// Spawn `command` through [`spawn`] and collect its output.
///
/// `Command::spawn` inherits standard streams by default, unlike
/// `Command::output`. Own all three streams here so callers keep the latter's
/// behavior while the signal mask is changed only for the spawn itself.
pub(crate) fn output(command: &mut Command) -> io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    spawn(command)?.wait_with_output()
}

/// Normalize a Waku-owned provider thread before a dependency spawns the child
/// internally. The ACP SDK owns its `async_process::Command`, so its dedicated
/// connection thread uses this once at startup instead of [`spawn`].
pub(crate) fn unblock_sigchld_for_current_thread() -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let sigchld = sigchld_set()?;
        pthread_result(unsafe {
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &sigchld, std::ptr::null_mut())
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

fn with_sigchld_unblocked<T>(operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    #[cfg(target_os = "macos")]
    let _restore = SignalMaskRestore::unblock_sigchld()?;
    operation()
}

#[cfg(target_os = "macos")]
fn sigchld_set() -> io::Result<libc::sigset_t> {
    let mut set = MaybeUninit::<libc::sigset_t>::uninit();
    if unsafe { libc::sigemptyset(set.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut set = unsafe { set.assume_init() };
    if unsafe { libc::sigaddset(&mut set, libc::SIGCHLD) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(set)
}

#[cfg(target_os = "macos")]
fn pthread_result(status: libc::c_int) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        // pthread APIs return the error number directly instead of setting
        // errno, so `last_os_error` would report unrelated thread state.
        Err(io::Error::from_raw_os_error(status))
    }
}

#[cfg(target_os = "macos")]
struct SignalMaskRestore(libc::sigset_t);

#[cfg(target_os = "macos")]
impl SignalMaskRestore {
    fn unblock_sigchld() -> io::Result<Self> {
        let sigchld = sigchld_set()?;
        let mut previous = MaybeUninit::<libc::sigset_t>::uninit();
        pthread_result(unsafe {
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &sigchld, previous.as_mut_ptr())
        })?;
        Ok(Self(unsafe { previous.assume_init() }))
    }
}

#[cfg(target_os = "macos")]
impl Drop for SignalMaskRestore {
    fn drop(&mut self) {
        let _ = unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &self.0, std::ptr::null_mut()) };
    }
}

pub fn find_executable(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 {
        return find_executable_at_path(candidate);
    }
    executable_search_paths()
        .into_iter()
        .flat_map(|directory| executable_candidates(&directory.join(name)))
        .find(|candidate| candidate.is_file())
}

fn find_executable_at_path(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    executable_candidates(path)
        .into_iter()
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
        return vec![path.to_path_buf()];
    }

    let mut candidates = extensions
        .map(|extension| {
            let mut candidate = path.as_os_str().to_os_string();
            candidate.push(extension);
            PathBuf::from(candidate)
        })
        .collect::<Vec<_>>();
    candidates.push(path.to_path_buf());
    candidates
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
        return find_executable_at_path(&candidate);
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

fn resolve_default_shell_path(timeout: Duration) -> Option<OsString> {
    let started_at = Instant::now();
    for shell in default_shell_candidates() {
        for shell_args in [["-i", "-l", "-c"].as_slice(), ["-l", "-c"].as_slice()] {
            let remaining = timeout.checked_sub(started_at.elapsed())?;
            if remaining.is_zero() {
                return None;
            }
            // Leave part of the total budget for a non-interactive login-shell
            // fallback when an interactive rc file blocks or exits early.
            let attempt_timeout = if shell_args.first() == Some(&"-i") {
                remaining.min(INTERACTIVE_SHELL_PATH_TIMEOUT)
            } else {
                remaining
            };
            if let Some(path) = capture_shell_path(&shell, shell_args, attempt_timeout) {
                return Some(path);
            }
        }
    }
    None
}

fn default_shell_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(shell) = std::env::var_os("SHELL").filter(|shell| !shell.is_empty()) {
        candidates.push(PathBuf::from(shell));
    }
    #[cfg(target_os = "macos")]
    if let Some(shell) = account_default_shell() {
        candidates.push(shell);
    }
    candidates.push(PathBuf::from("/bin/zsh"));

    let mut seen = HashSet::new();
    candidates.retain(|shell| seen.insert(shell.clone()));
    candidates
}

#[cfg(target_os = "macos")]
fn account_default_shell() -> Option<PathBuf> {
    let suggested_size = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut buffer_size = if suggested_size > 0 {
        suggested_size as usize
    } else {
        16 * 1024
    };
    loop {
        let mut passwd = MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0_u8; buffer_size];
        let status = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                passwd.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && buffer_size < 1024 * 1024 {
            buffer_size *= 2;
            continue;
        }
        if status != 0 || result.is_null() {
            return None;
        }
        let shell = unsafe { (*result).pw_shell };
        if shell.is_null() {
            return None;
        }
        let bytes = unsafe { CStr::from_ptr(shell) }.to_bytes();
        return (!bytes.is_empty()).then(|| PathBuf::from(OsString::from_vec(bytes.to_vec())));
    }
}

fn capture_shell_path(shell: &Path, shell_args: &[&str], timeout: Duration) -> Option<OsString> {
    let capture = ShellPathCapture::create()?;
    let mut command = Command::new(shell);
    command
        .args(shell_args)
        .arg(SHELL_PATH_COMMAND)
        .env("WAKU_SHELL_PATH_CAPTURE_FILE", capture.path())
        // Match shell-env's safeguards for common interactive zsh setups so
        // an update prompt or tmux auto-start cannot consume the probe budget.
        .env("DISABLE_AUTO_UPDATE", "true")
        .env("ZSH_TMUX_AUTOSTARTED", "true")
        .env("ZSH_TMUX_AUTOSTART", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "macos")]
    command.process_group(0);

    let mut child = spawn(&mut command).ok()?;
    if !wait_for_child(&mut child, timeout) {
        return None;
    }
    let mut bytes = fs::read(capture.path()).ok()?;
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    if bytes.is_empty() {
        return None;
    }
    #[cfg(target_os = "macos")]
    return Some(OsString::from_vec(bytes));
    #[cfg(not(target_os = "macos"))]
    return String::from_utf8(bytes).ok().map(OsString::from);
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> bool {
    let started_at = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started_at.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                terminate_shell_capture(child);
                return false;
            }
        }
    }
}

fn terminate_shell_capture(child: &mut Child) {
    #[cfg(target_os = "macos")]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

struct ShellPathCapture(PathBuf);

impl ShellPathCapture {
    fn create() -> Option<Self> {
        for _ in 0..16 {
            let id = SHELL_PATH_CAPTURE_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!(".waku-shell-path-{}-{id}", std::process::id()));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(target_os = "macos")]
            options.mode(0o600);
            match options.open(&path) {
                Ok(_) => return Some(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return None,
            }
        }
        None
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ShellPathCapture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn output_captures_stdout_and_stderr() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf stdout; printf stderr >&2"]);

        let output = output(&mut command).expect("command should run");

        assert_eq!(output.stdout, b"stdout");
        assert_eq!(output.stderr, b"stderr");
    }

    #[cfg(target_os = "macos")]
    fn sigchld_is_blocked() -> io::Result<bool> {
        let mut current = MaybeUninit::<libc::sigset_t>::uninit();
        pthread_result(unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), current.as_mut_ptr())
        })?;
        Ok(unsafe { libc::sigismember(current.as_ptr(), libc::SIGCHLD) } == 1)
    }

    #[cfg(target_os = "macos")]
    fn block_sigchld() -> io::Result<SignalMaskRestore> {
        let sigchld = sigchld_set()?;
        let mut previous = MaybeUninit::<libc::sigset_t>::uninit();
        pthread_result(unsafe {
            libc::pthread_sigmask(libc::SIG_BLOCK, &sigchld, previous.as_mut_ptr())
        })?;
        Ok(SignalMaskRestore(unsafe { previous.assume_init() }))
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn spawn_unblocks_sigchld_in_the_child_and_restores_the_caller() {
        if std::env::var_os("WAKU_SIGCHLD_CHILD_PROBE").is_some() {
            assert!(!sigchld_is_blocked().expect("read child signal mask"));
            return;
        }

        let _restore_original = block_sigchld().expect("block SIGCHLD for the fixture");
        assert!(sigchld_is_blocked().expect("read blocked parent mask"));

        let mut command = Command::new(std::env::current_exe().expect("resolve test executable"));
        command
            .args([
                "--exact",
                "command_env::tests::spawn_unblocks_sigchld_in_the_child_and_restores_the_caller",
                "--nocapture",
            ])
            .env("WAKU_SIGCHLD_CHILD_PROBE", "1");
        let output = output(&mut command).expect("spawn child signal probe");

        assert!(
            output.status.success(),
            "child signal probe failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(sigchld_is_blocked().expect("read restored parent mask"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dedicated_provider_thread_can_normalize_sigchld() {
        let _restore_original = block_sigchld().expect("block SIGCHLD for the fixture");

        unblock_sigchld_for_current_thread().expect("unblock provider thread");

        assert!(!sigchld_is_blocked().expect("read normalized signal mask"));
    }

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
        assert_eq!(
            executable_candidates_with_extensions(Path::new("tool"), ".COM;.EXE;.BAT;.CMD"),
            [
                PathBuf::from("tool.COM"),
                PathBuf::from("tool.EXE"),
                PathBuf::from("tool.BAT"),
                PathBuf::from("tool.CMD"),
                PathBuf::from("tool"),
            ]
        );
        assert_eq!(
            executable_candidates_with_extensions(Path::new("tool.eXe"), ".COM;.EXE;.BAT;.CMD"),
            [PathBuf::from("tool.eXe")]
        );
    }

    #[cfg(windows)]
    #[test]
    fn explicit_paths_keep_existing_custom_extensions() {
        let root = std::env::temp_dir().join(format!("waku-command-env-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let original = root.join("agent.bin");
        let sibling = root.join("agent.bin.exe");
        std::fs::write(&original, b"custom launcher").unwrap();
        std::fs::write(&sibling, b"native launcher").unwrap();

        assert_eq!(find_executable_at_path(&original), Some(original.clone()));

        let _ = std::fs::remove_dir_all(root);
    }
}
