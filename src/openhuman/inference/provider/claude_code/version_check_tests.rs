use super::*;

#[test]
fn parses_typical_output() {
    assert_eq!(
        parse_version("2.0.4 (Claude Code)\n").as_deref(),
        Some("2.0.4")
    );
}

#[test]
fn rejects_non_numeric_prefix() {
    assert_eq!(parse_version("claude version 2.0.4"), None);
}

#[test]
fn version_compare() {
    assert!(version_lt("1.9.9", "2.0.0"));
    assert!(version_lt("2.0.0", "2.0.1"));
    assert!(!version_lt("2.0.0", "2.0.0"));
    assert!(!version_lt("2.1.0", "2.0.9"));
}

#[test]
fn version_compare_strips_prerelease() {
    assert!(!version_lt("2.0.0-rc.1", "2.0.0"));
}

/// A macOS app launched from Finder gets launchd's minimal `PATH`, so the
/// native-installer location must be probed directly — it is the default
/// install route and the one that regressed in the field.
#[test]
fn well_known_candidates_cover_the_native_installer_and_homebrew() {
    let home = Path::new("/Users/someone");
    let candidates = super::well_known_candidates(Some(home));

    assert_eq!(candidates.first(), Some(&home.join(".local/bin/claude")));
    assert!(candidates.contains(&PathBuf::from("/opt/homebrew/bin/claude")));
    assert!(candidates.contains(&PathBuf::from("/usr/local/bin/claude")));
}

#[test]
fn well_known_candidates_without_a_home_still_probe_system_prefixes() {
    let candidates = super::well_known_candidates(None);

    assert_eq!(
        candidates,
        vec![
            PathBuf::from("/opt/homebrew/bin/claude"),
            PathBuf::from("/usr/local/bin/claude"),
        ]
    );
}

/// The bound is the whole point of the login-shell probe: `-lc` sources the
/// user's rc files, and one that blocks — on a prompt, on a slow network call —
/// would otherwise hang provider construction forever with no diagnostic.
/// Pointed at a "shell" that never returns, the probe must give up, not wait.
#[test]
fn a_blocking_login_shell_is_abandoned_rather_than_waited_on() {
    let dir = tempfile::tempdir().expect("tempdir");
    let shell = dir.path().join("blocking-shell");
    std::fs::write(&shell, "#!/bin/sh\nsleep 30\n").expect("write shell");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755))
            .expect("chmod shell");
    }

    let started = std::time::Instant::now();
    let resolved = super::login_shell_lookup_with(
        shell.to_str().expect("utf8 path"),
        Duration::from_millis(200),
    );
    let elapsed = started.elapsed();

    assert_eq!(
        resolved, None,
        "a shell that never answers resolves nothing"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "probe waited {elapsed:?}; the budget was not honoured"
    );
}

/// How long the probe under test is allowed to wait before it gives up.
///
/// Deliberately longer than [`PID_DEADLINE`]. These two used to be equal, and
/// that is a race the test loses under load: the shell has to start, source the
/// container's profile scripts and reach its first `echo` before the probe's
/// own budget expires and kills it. On a loaded CI runner, under llvm-cov
/// instrumentation, it does not always win — and the test then fails with "the
/// shell never recorded its pid", which is a statement about the runner rather
/// than about the code. The probe must outlast the observation window.
const PROBE_BUDGET: Duration = Duration::from_secs(6);

/// How long the test waits for the shell to announce itself before giving up.
const PID_DEADLINE: Duration = Duration::from_secs(4);

/// The timeout must reap the shell, not merely stop waiting for it.
///
/// The reason the child is spawned on the calling thread rather than handed to
/// `Command::output()` is that a probe which only abandons its worker leaves
/// the shell running for the life of the app. The script records its own pid
/// before blocking, so this asserts the process is actually gone afterwards
/// rather than asserting the shape of the code.
#[cfg(unix)]
#[test]
fn a_timed_out_login_shell_is_killed_not_merely_abandoned() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("pid");
    let shell = dir.path().join("blocking-shell");
    std::fs::write(
        &shell,
        format!("#!/bin/sh\necho $$ > {}\nsleep 30\n", pid_file.display()),
    )
    .expect("write shell");
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod shell");

    // The probe runs on its own thread so the test can wait for the shell to
    // record its pid BEFORE the budget expires. Reading the file afterwards
    // would be a race: a shell killed before it got to `echo` never writes one,
    // and the test would fail for a reason that is not the defect.
    let shell_path = shell.to_str().expect("utf8 path").to_string();
    let probe =
        std::thread::spawn(move || super::login_shell_lookup_with(&shell_path, PROBE_BUDGET));

    let deadline = std::time::Instant::now() + PID_DEADLINE;
    let pid = loop {
        if let Ok(raw) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = raw.trim().parse::<i32>() {
                break pid;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the shell never recorded its pid"
        );
        std::thread::sleep(Duration::from_millis(20));
    };

    assert_eq!(
        probe.join().expect("probe thread"),
        None,
        "a blocking shell resolves nothing"
    );

    assert!(!is_running(pid), "shell pid {pid} survived the timeout");
}

/// K2: the flags are the whole reason this fallback finds anything.
///
/// `-i` is what makes zsh read `.zshrc` and bash read `.bashrc`, which is where
/// nvm/mise/asdf install their init — drop it and the probe silently misses
/// exactly the layouts it exists to cover, while still compiling and still
/// returning `None` on every machine. So assert the argv the shell actually
/// receives, not the argv the source appears to pass.
#[cfg(unix)]
#[test]
fn the_login_shell_is_asked_as_an_interactive_login_shell() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let argv_file = dir.path().join("argv");
    // The probe discards anything that is not an existing file, so the fake
    // shell must answer with a real path for the success arm to be exercised.
    let resolved = dir.path().join("claude");
    std::fs::write(&resolved, "#!/bin/sh\n").expect("write claude");

    let shell = dir.path().join("recording-shell");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\necho {}\n",
            argv_file.display(),
            resolved.display()
        ),
    )
    .expect("write shell");
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod shell");

    let found =
        super::login_shell_lookup_with(shell.to_str().expect("utf8 path"), Duration::from_secs(10));
    assert_eq!(found.as_deref(), Some(resolved.as_path()));

    let argv = std::fs::read_to_string(&argv_file).expect("shell recorded its argv");
    assert_eq!(
        argv.lines().collect::<Vec<_>>(),
        vec!["-lic", "command -v claude"],
        "the probe must ask an interactive login shell"
    );
}

/// EOF on stdout is not the shell exiting.
///
/// A profile that closes stdout and then sleeps sends its output immediately,
/// so the read returns at once and the `wait()` that follows had no deadline —
/// `from_env` blocked for as long as the shell chose to sleep. The bound now
/// covers both, so this returns well inside the sleep.
#[cfg(unix)]
#[test]
fn a_shell_that_closes_stdout_and_sleeps_does_not_hold_the_probe() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let resolved = dir.path().join("claude");
    std::fs::write(&resolved, "#!/bin/sh\n").expect("write claude");

    let shell = dir.path().join("lingering-shell");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\necho {}\nexec 1>&-\nsleep 30\n",
            resolved.display()
        ),
    )
    .expect("write shell");
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod shell");

    let started = std::time::Instant::now();
    let found =
        super::login_shell_lookup_with(shell.to_str().expect("utf8 path"), Duration::from_secs(2));
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(10),
        "the probe must be bounded by its own budget, not by the shell's sleep (took {elapsed:?})"
    );
    // The shell never exits within the budget, so there is no successful status
    // to trust — the probe reports nothing rather than a path it cannot vouch
    // for. `found` is asserted only to pin that it returns at all.
    assert!(found.is_none() || found.as_deref() == Some(resolved.as_path()));
}

/// An rc file that greets the user puts its banner on the same stdout as
/// `command -v`. Reading all of it as one path resolves nothing, and the
/// installed CLI is reported absent — a regression `-lic` made reachable.
#[cfg(unix)]
#[test]
fn a_banner_printed_by_the_rc_files_does_not_hide_the_cli() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let resolved = dir.path().join("claude");
    std::fs::write(&resolved, "#!/bin/sh\n").expect("write claude");

    let shell = dir.path().join("chatty-shell");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\necho 'Welcome back!'\necho ''\necho {}\n",
            resolved.display()
        ),
    )
    .expect("write shell");
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod shell");

    let found =
        super::login_shell_lookup_with(shell.to_str().expect("utf8 path"), Duration::from_secs(10));
    assert_eq!(
        found.as_deref(),
        Some(resolved.as_path()),
        "the last non-empty line is the answer; the banner above it is not"
    );
}

/// K3: the bound must reap the whole tree, not just the shell.
///
/// An rc file that backgrounds anything — and `-i` means rc files run — leaves
/// a grandchild that `Child::kill` never touches. Reparented to init, it
/// outlives the app that spawned it. The fake rc records the background pid so
/// this asserts the process is really gone rather than asserting code shape.
#[cfg(unix)]
#[test]
fn a_timed_out_login_shell_takes_its_grandchildren_with_it() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let child_pid_file = dir.path().join("child.pid");
    let shell = dir.path().join("backgrounding-shell");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nsleep 30 &\necho $! > {}\nsleep 30\n",
            child_pid_file.display()
        ),
    )
    .expect("write shell");
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod shell");

    // Same reason as the test above: read the pid BEFORE the budget expires,
    // otherwise a shell killed early never writes one and the test fails for a
    // reason that is not the defect.
    let shell_path = shell.to_str().expect("utf8 path").to_string();
    let probe =
        std::thread::spawn(move || super::login_shell_lookup_with(&shell_path, PROBE_BUDGET));

    let deadline = std::time::Instant::now() + PID_DEADLINE;
    let grandchild = loop {
        if let Ok(raw) = std::fs::read_to_string(&child_pid_file) {
            if let Ok(pid) = raw.trim().parse::<i32>() {
                break pid;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the rc file never recorded its background pid"
        );
        std::thread::sleep(Duration::from_millis(20));
    };

    assert_eq!(
        probe.join().expect("probe thread"),
        None,
        "a blocking shell resolves nothing"
    );

    let gone_by = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        if !is_running(grandchild) {
            break;
        }
        assert!(
            std::time::Instant::now() < gone_by,
            "grandchild pid {grandchild} outlived the probe as an orphan"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether `pid` names a process that is still *running*, as opposed to one
/// that is dead but not yet reaped.
///
/// `kill -0` cannot answer this: it succeeds for a zombie, because a zombie
/// still owns its pid until someone waits on it. That distinction does not
/// matter on a desktop, where the orphan is reparented to a PID 1 that reaps
/// it within milliseconds — and it decides the test inside CI's container,
/// where PID 1 is the job's own command and reaps nothing. The killed
/// grandchild sits there as a zombie and `kill -0` reports it alive forever.
///
/// So ask for the state instead. `ps` prints `Z` for a zombie on both Linux and
/// macOS, and prints nothing at all once the process is gone.
#[cfg(unix)]
fn is_running(pid: i32) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "state=", "-p", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .output()
        .expect("run ps");
    let state = String::from_utf8_lossy(&out.stdout);
    let state = state.trim();
    !state.is_empty() && !state.starts_with('Z')
}

/// A shell that exits 0 still leaves its process group behind if the rc file
/// backgrounded something with its output redirected: stdout hits EOF, the
/// answer arrives, `wait_until` reaps the shell, and the sleeper keeps running
/// with nothing holding a handle to it. The success path has to signal the
/// group too, not only the timeout path.
#[cfg(unix)]
#[test]
fn a_successful_login_shell_takes_its_grandchildren_with_it() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let child_pid_file = dir.path().join("child.pid");
    // The probe only returns a path that `is_file()`, so answer with one.
    let answer = dir.path().join("claude");
    std::fs::write(&answer, "#!/bin/sh\n").expect("write answer");

    let shell = dir.path().join("backgrounding-shell");
    std::fs::write(
        &shell,
        format!(
            // The redirect is the point: without it the sleeper holds stdout
            // open and the probe times out instead of succeeding.
            "#!/bin/sh\nsleep 30 >/dev/null 2>&1 &\necho $! > {}\necho {}\n",
            child_pid_file.display(),
            answer.display()
        ),
    )
    .expect("write shell");
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod shell");

    let shell_path = shell.to_str().expect("utf8 path").to_string();
    let probe =
        std::thread::spawn(move || super::login_shell_lookup_with(&shell_path, PROBE_BUDGET));

    let deadline = std::time::Instant::now() + PID_DEADLINE;
    let grandchild = loop {
        if let Ok(raw) = std::fs::read_to_string(&child_pid_file) {
            if let Ok(pid) = raw.trim().parse::<i32>() {
                break pid;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the rc file never recorded its background pid"
        );
        std::thread::sleep(Duration::from_millis(20));
    };

    assert_eq!(
        probe.join().expect("probe thread"),
        Some(answer),
        "the shell exited 0 and printed a real path"
    );

    let gone_by = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        if !is_running(grandchild) {
            break;
        }
        assert!(
            std::time::Instant::now() < gone_by,
            "grandchild pid {grandchild} outlived a successful probe as an orphan"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
