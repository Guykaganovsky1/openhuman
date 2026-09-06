//! Locate the `claude` CLI binary and verify it meets `MIN_CLI_VERSION`.
//!
//! We rely on `claude --version`, which prints a line of the form:
//!   `2.0.4 (Claude Code)`
//! The first whitespace-delimited token is the semver string we compare
//! against [`MIN_CLI_VERSION`].

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::types::{CliStatus, MIN_CLI_VERSION};

/// How long the login-shell probe may take before it is abandoned.
const LOGIN_SHELL_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Locate the `claude` CLI binary on `PATH`.
///
/// Honors `OPENHUMAN_CLAUDE_CLI` env override so tests and power users can
/// point at a specific binary.
pub fn resolve_binary() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("OPENHUMAN_CLAUDE_CLI") {
        let p = PathBuf::from(explicit);
        if p.exists() {
            return Some(p);
        }
    }
    which_on_path("claude").or_else(well_known_install)
}

/// Fallback locations for the `claude` CLI, probed when `PATH` does not carry
/// it.
///
/// A macOS app launched from Finder/Dock inherits `launchd`'s minimal `PATH`
/// (`/usr/bin:/bin:/usr/sbin:/sbin`), **not** the login shell's — so the same
/// install that resolves fine from a terminal-launched build reports
/// `NotInstalled` in the shipped app. The npm-global, Homebrew and native
/// installer locations below cover every documented install route; a
/// version-manager layout (nvm, asdf, mise) is not on that list and is picked
/// up by the time-boxed login-shell probe that follows it.
fn well_known_install() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    for candidate in well_known_candidates(home.as_deref()) {
        if candidate.is_file() {
            log::debug!(
                "[claude-code][version] resolved off-PATH candidate path={}",
                candidate.display()
            );
            return Some(candidate);
        }
    }

    login_shell_lookup()
}

/// Ask the user's login shell where `claude` lives — last resort, time-boxed.
///
/// The fixed list above cannot cover a version-manager layout: nvm puts an
/// npm-global binary under `~/.nvm/versions/node/<version>/bin`, and asdf/mise
/// resolve through shims whose path is decided by the shell's own init. Those
/// are real installs, so dropping this fallback would regress users who could
/// resolve the CLI before.
///
/// It is bounded because `-lc` sources the user's rc files, and an rc file that
/// blocks — on a prompt, on a slow network call — would otherwise hang provider
/// construction forever with no diagnostic. Two seconds is far longer than
/// `command -v` needs and far shorter than a user will wait.
///
/// `-lic`, not `-lc`: a login shell alone is the wrong shell. zsh reads
/// `.zprofile`/`.zlogin` when it is a login shell and `.zshrc` only when it is
/// interactive, and bash reads `.bash_profile` versus `.bashrc` the same way —
/// and nvm, mise and asdf overwhelmingly install their init into the
/// *interactive* file. Without `-i` this fallback misses exactly the layouts it
/// exists to cover. `-i` is safe here because the bound above is enforced by
/// killing the child, so an rc file that waits on a tty costs one timeout
/// rather than a hung process.
///
/// `command -v` rather than `which`: it is POSIX-builtin and resolves the way
/// the user's own terminal would. A shell *function* named `claude` (a common
/// wrapper) makes it print the function body instead of a path, so anything
/// that is not an existing file is discarded rather than handed to
/// `Command::new`.
fn login_shell_lookup() -> Option<PathBuf> {
    // Resolved at most ONCE per process, and this is load-bearing, not an
    // optimisation. `probe()` is uncached and runs on every turn build, on the
    // tokio worker driving that turn (`TurnModelSource::build` is sync all the
    // way down). Without this cache a machine whose shell profile is slow would
    // pay the full budget on every turn AND abandon one worker thread plus one
    // shell process each time, unbounded, for the life of the app. Cached, the
    // worst case is one stalled thread and one 2s wait, once.
    //
    // Only this fallback is cached — not `probe()` — so a user who installs the
    // CLI onto `PATH`, or into any of the well-known directories above, is
    // still picked up on the next turn without a restart. Those are re-probed
    // every time; it is the shell question alone that is asked once.
    //
    // The accepted cost is a sticky negative: a user whose CLI arrives *only*
    // through a version manager (nvm/asdf/mise) mid-session keeps the cached
    // `None` until the app restarts. That is the deliberate trade — the
    // alternative is re-asking a shell that already proved slow, on every turn,
    // abandoning a thread and a shell process each time with no bound. A
    // restart is a fair price for the rarer half of an already-rare path, and a
    // removed or downgraded binary is NOT affected: `probe()` still runs
    // `--version` against the cached path on every turn, so it degrades to
    // `Unusable`/`NotInstalled` with the right message rather than going stale.
    static RESOLVED: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

    RESOLVED
        .get_or_init(|| {
            if cfg!(windows) {
                return None;
            }
            let shell = std::env::var("SHELL")
                .ok()
                .filter(|s| !s.trim().is_empty())?;
            login_shell_lookup_with(&shell, LOGIN_SHELL_PROBE_TIMEOUT)
        })
        .clone()
}

/// The probe itself, with the shell and the budget passed in.
///
/// Split out so the timeout branch is testable: a test can point this at a
/// script that never returns and assert it gives up, which is the whole reason
/// the bound exists. Reading `SHELL` inside would have forced an env-mutating
/// test that races every other test in the binary.
fn login_shell_lookup_with(shell: &str, budget: Duration) -> Option<PathBuf> {
    // The child is spawned HERE, on the calling thread, and never moved into
    // the reader thread — that is the whole point. `Command::output()` would
    // give the handle away, leaving nothing to kill when the budget expires, so
    // a shell that blocks in an rc file would survive as an orphan process for
    // the life of the app. Only the stdout pipe crosses the thread boundary;
    // killing the child closes it, which ends the reader on its own.
    let mut command = Command::new(shell);
    command
        .args(["-lic", "command -v claude"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    // Its own process group, so the budget below can reap the whole tree.
    // `-i` sources the user's interactive rc files, and one that backgrounds
    // anything (`sleep 30 &`, a daemon warm-up, a version-manager prefetch)
    // leaves a grandchild that killing the shell alone never touches — it is
    // reparented to init and survives for the life of the app, which is the
    // very leak the bound exists to prevent.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|err| {
            log::debug!("[claude-code][version] login shell probe failed err={err}");
        })
        .ok()?;

    let mut pipe = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut out = String::new();
        let _ = pipe.read_to_string(&mut out);
        let _ = tx.send(out);
    });

    // One deadline for the whole probe. EOF on stdout is not exit: a profile
    // that closes stdout and then sleeps sends `out` immediately and would have
    // left the `wait()` below unbounded, blocking startup on the shell.
    let deadline = std::time::Instant::now() + budget;
    let stdout = match rx.recv_timeout(budget) {
        Ok(out) => out,
        Err(_) => {
            log::warn!(
                "[claude-code][version] login shell probe timed out after {}s; \
                 a slow or blocking shell profile can cause this",
                budget.as_secs()
            );
            kill_probe_tree(&mut child);
            let _ = child.wait();
            return None;
        }
    };

    // Captured before the reap: `wait_until` consumes the child, and reading
    // the pid off a reaped `Child` is reading a pid the kernel is free to hand
    // to someone else.
    #[cfg(unix)]
    let pgid = child.id() as libc::pid_t;

    match wait_until(&mut child, deadline) {
        Some(status) if status.success() => {
            // The shell exited, but `process_group(0)` gave it a group of its
            // own, and a profile that backgrounds something with stdout
            // redirected leaves that child running in it — stdout hit EOF, so
            // nothing above noticed. Signal the group so the probe does not
            // outlive itself.
            #[cfg(unix)]
            kill_probe_group(pgid);
        }
        Some(_) => return None,
        None => {
            log::warn!(
                "[claude-code][version] login shell answered but did not exit within {}s; \
                 killing the probe",
                budget.as_secs()
            );
            kill_probe_tree(&mut child);
            let _ = child.wait();
            return None;
        }
    }
    // An interactive login shell runs the user's rc files, and one that prints
    // a banner puts that text on the same stdout as `command -v`. Only the last
    // non-empty line is the answer; joining them yields a path that no
    // `is_file()` accepts, reporting an installed CLI as absent.
    let path = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)?;
    path.is_file().then(|| {
        log::debug!(
            "[claude-code][version] resolved via login shell path={}",
            path.display()
        );
        path
    })
}

/// Wait for the probe to exit, giving up at `deadline`.
///
/// `std::process::Child` has no timed wait, so this polls. The interval is the
/// usual trade: short enough that the common case (already exited) costs one
/// poll, long enough not to spin.
fn wait_until(
    child: &mut std::process::Child,
    deadline: std::time::Instant,
) -> Option<std::process::ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_) => return None,
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Kill the timed-out probe and everything its rc files started.
///
/// The child is its own process-group leader (see `process_group(0)` above), so
/// a negative pid signals the whole group. `Child::kill` alone reaps only the
/// shell: a `sleep` backgrounded from `.zshrc` outlives it as an orphan, which
/// is exactly what the 2s bound is supposed to prevent.
#[cfg(unix)]
fn kill_probe_tree(child: &mut std::process::Child) {
    kill_probe_group(child.id() as libc::pid_t);
}

/// SIGKILL the process group led by `pgid`.
///
/// `kill(2)` with a negative pid signals a whole group, which is the point:
/// the probe shell is its own group leader (`process_group(0)`), so this
/// reaches anything it started and left behind.
#[cfg(unix)]
fn kill_probe_group(pgid: libc::pid_t) {
    log::debug!("[claude-code][version] killing login shell process group pgid={pgid}");
    // SAFETY: signalling a process group is safe; the worst case is ESRCH for
    // a group that is already gone, which is ignored.
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_probe_tree(child: &mut std::process::Child) {
    let _ = child.kill();
}

/// The ordered fallback candidates, split out so the list is unit-testable
/// without mutating the process environment.
fn well_known_candidates(home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = home {
        for suffix in [
            ".local/bin/claude",
            ".claude/local/claude",
            ".bun/bin/claude",
            ".volta/bin/claude",
            "Library/pnpm/claude",
            ".npm-global/bin/claude",
        ] {
            candidates.push(home.join(suffix));
        }
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/claude"));
    candidates.push(PathBuf::from("/usr/local/bin/claude"));
    candidates
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path_var) {
        if cfg!(windows) {
            for ext in &exts {
                let candidate = dir.join(format!("{name}{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        } else {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Probe the `claude` CLI and return its status.
pub fn probe() -> CliStatus {
    let Some(path) = resolve_binary() else {
        log::debug!("[claude-code][version] no `claude` binary on PATH");
        return CliStatus::NotInstalled;
    };
    let path_str = path.display().to_string();

    let output = match Command::new(&path).arg("--version").output() {
        Ok(o) => o,
        Err(e) => {
            log::warn!("[claude-code][version] spawn failed path={path_str} err={e}");
            return CliStatus::Unusable {
                path: path_str,
                reason: format!("spawn failed: {e}"),
            };
        }
    };

    if !output.status.success() {
        return CliStatus::Unusable {
            path: path_str,
            reason: format!(
                "non-zero exit {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        };
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = match parse_version(&stdout) {
        Some(v) => v,
        None => {
            return CliStatus::Unusable {
                path: path_str,
                reason: format!("could not parse version from: {stdout:?}"),
            }
        }
    };

    if version_lt(&version, MIN_CLI_VERSION) {
        CliStatus::Outdated {
            version,
            min_required: MIN_CLI_VERSION.to_string(),
            path: path_str,
        }
    } else {
        CliStatus::Ok {
            version,
            path: path_str,
        }
    }
}

fn parse_version(stdout: &str) -> Option<String> {
    stdout
        .split_whitespace()
        .next()
        .filter(|tok| tok.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|s| s.to_string())
}

/// Numeric semver compare. Returns true when `a < b`.
/// Pre-release suffixes (`-rc.1`) are stripped before comparison.
fn version_lt(a: &str, b: &str) -> bool {
    let pa = parts(a);
    let pb = parts(b);
    pa < pb
}

fn parts(v: &str) -> (u32, u32, u32) {
    let core = v.split('-').next().unwrap_or(v);
    let mut it = core.split('.').map(|s| s.parse::<u32>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

#[cfg(test)]
#[path = "version_check_tests.rs"]
mod tests;
