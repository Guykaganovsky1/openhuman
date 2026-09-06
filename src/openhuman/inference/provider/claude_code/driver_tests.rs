use super::*;

#[test]
fn write_mcp_http_config_emits_http_url_with_bearer_header() {
    let dir = tempfile::tempdir().expect("tempdir");
    let addr: std::net::SocketAddr = "127.0.0.1:54321".parse().unwrap();
    let path = write_mcp_http_config(dir.path(), addr, "tok-abc123").expect("write config");
    let raw = std::fs::read_to_string(&path).expect("read config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
    let server = &v["mcpServers"]["openhuman"];
    assert_eq!(
        server["type"], "http",
        "MCP transport must be http (out-of-jail)"
    );
    assert_eq!(server["url"], "http://127.0.0.1:54321/");
    // The loopback server is authenticated — the config must carry the bearer.
    assert_eq!(server["headers"]["Authorization"], "Bearer tok-abc123");
    // It must NOT spawn a stdio child (the old jailed path).
    assert!(server.get("command").is_none());
}

#[test]
fn large_system_prompt_is_written_to_file_instead_of_argv() {
    let dir = tempfile::tempdir().expect("tempdir");
    let prompt = "system instruction\n".repeat(2_500);
    assert!(prompt.len() > 32_767);

    let args = append_system_prompt_args(dir.path(), Some(&prompt)).expect("prompt args");

    assert_eq!(args[0], "--append-system-prompt-file");
    assert_eq!(args.len(), 2);
    assert!(!args.iter().any(|arg| arg.contains(&prompt)));
    assert_eq!(
        std::fs::read_to_string(&args[1]).expect("read prompt file"),
        prompt
    );
}

#[test]
fn empty_system_prompt_does_not_add_an_argument() {
    let dir = tempfile::tempdir().expect("tempdir");
    let args = append_system_prompt_args(dir.path(), Some("  \n ")).expect("prompt args");

    assert!(args.is_empty());
    assert!(!dir.path().join("append-system-prompt.txt").exists());
}

#[test]
fn system_prompt_write_error_is_propagated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let not_a_directory = dir.path().join("file");
    std::fs::write(&not_a_directory, "occupied").expect("write blocking file");

    let error = append_system_prompt_args(&not_a_directory, Some("system prompt"))
        .expect_err("non-directory parent must fail");

    assert!(!error.to_string().is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn seatbelt_profile_denies_whole_openhuman_root_not_just_subdir() {
    // Driver passes the per-user subdir; the jail must deny the WHOLE
    // `.openhuman-staging` tree (so root-level core.token/credentials are
    // protected), not just the subdir.
    let ws = std::path::Path::new("/Users/test/.openhuman-staging/users/abc/workspace");
    let p = seatbelt_profile(ws);
    assert!(
        p.contains("(allow default)"),
        "CC does everything by default"
    );
    assert!(p.contains("(deny file-write*"), "must deny writes");
    assert!(
        p.contains("(deny file-read*"),
        "must deny reads (no token exfil)"
    );
    // Denied path is the ROOT, not the per-user subdir.
    assert!(
        p.contains("/Users/test/.openhuman-staging\""),
        "deny subpath must be the .openhuman root: {p}"
    );
    assert!(
        !p.contains("users/abc"),
        "deny must NOT be scoped to the narrow subdir: {p}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn openhuman_internal_root_walks_up_to_dotopenhuman() {
    let r = openhuman_internal_root(std::path::Path::new(
        "/Users/x/.openhuman/users/id/workspace/memory",
    ));
    assert_eq!(r, std::path::Path::new("/Users/x/.openhuman"));
    // Fallback: no `.openhuman*` ancestor → returns the input.
    let r2 = openhuman_internal_root(std::path::Path::new("/tmp/custom/ws"));
    assert_eq!(r2, std::path::Path::new("/tmp/custom/ws"));
}

#[cfg(target_os = "macos")]
#[test]
fn seatbelt_available_honors_opt_out() {
    let _env = super::super::ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("OPENHUMAN_CLAUDE_CODE_SANDBOX").ok();
    std::env::set_var("OPENHUMAN_CLAUDE_CODE_SANDBOX", "0");
    assert!(
        !seatbelt_available(),
        "explicit opt-out must disable the jail"
    );
    match prev {
        Some(v) => std::env::set_var("OPENHUMAN_CLAUDE_CODE_SANDBOX", v),
        None => std::env::remove_var("OPENHUMAN_CLAUDE_CODE_SANDBOX"),
    }
}

#[test]
fn full_access_defaults_off_and_opts_in_via_env() {
    let _env = super::super::ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // Empty workspace (no persisted toggle) → file layer resolves to OFF.
    let ws = std::env::temp_dir().join("oh_cc_fullaccess_env_test");
    let _ = std::fs::remove_dir_all(&ws);
    let key = "OPENHUMAN_CLAUDE_CODE_PERMISSION_MODE";
    let prev = std::env::var(key).ok();
    std::env::remove_var(key);
    assert!(
        !claude_code_full_access(&ws),
        "default posture must be acceptEdits (full access OFF)"
    );
    std::env::set_var(key, "bypass");
    assert!(
        claude_code_full_access(&ws),
        "explicit opt-in (`bypass`) enables full access"
    );
    std::env::set_var(key, "acceptEdits");
    assert!(
        !claude_code_full_access(&ws),
        "acceptEdits env override keeps the default (limited) posture"
    );
    match prev {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
}

#[test]
fn full_access_reads_persisted_toggle_when_env_unset() {
    use super::super::settings::{self, ClaudeCodeSettings};
    let _env = super::super::ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let ws = std::env::temp_dir().join("oh_cc_fullaccess_file_test");
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).unwrap();
    let key = "OPENHUMAN_CLAUDE_CODE_PERMISSION_MODE";
    let prev = std::env::var(key).ok();
    std::env::remove_var(key);

    settings::save(&ws, &ClaudeCodeSettings { full_access: true }).unwrap();
    assert!(
        claude_code_full_access(&ws),
        "persisted toggle ON must enable full access when env is unset"
    );

    // Env override beats the persisted toggle.
    std::env::set_var(key, "acceptEdits");
    assert!(
        !claude_code_full_access(&ws),
        "env override OFF must beat a persisted ON toggle"
    );

    match prev {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
    let _ = std::fs::remove_dir_all(&ws);
}

/// The turn budget is the difference between "the CLI is still working" and
/// "the CLI is broken", so its parse rules are worth pinning. Exercised through
/// the pure helper rather than the env var: an env-mutating test races every
/// other test in this binary.
#[test]
fn turn_timeout_defaults_when_unset_or_unparseable() {
    let default = Duration::from_secs(DEFAULT_TURN_TIMEOUT_SECS);

    assert_eq!(parse_turn_timeout(None), default);
    assert_eq!(parse_turn_timeout(Some("")), default);
    assert_eq!(parse_turn_timeout(Some("not-a-number")), default);
    assert_eq!(parse_turn_timeout(Some("-30")), default);
}

/// Zero is rejected rather than honoured — it would kill every child the
/// instant it started, which reads as a broken CLI rather than a bad setting.
#[test]
fn turn_timeout_rejects_zero_and_honours_a_real_override() {
    assert_eq!(
        parse_turn_timeout(Some("0")),
        Duration::from_secs(DEFAULT_TURN_TIMEOUT_SECS)
    );
    assert_eq!(parse_turn_timeout(Some("  120 ")), Duration::from_secs(120));
}

/// Only a spawn failure meaning "this binary is unusable" may carry the marker,
/// because the marker classifies as a NON-RETRYABLE setup problem. ETXTBSY (the
/// binary is being rewritten) and EAGAIN (fork pressure) are transient, and
/// telling the user to reinstall would both mislead and suppress the retry that
/// would have worked. The question is answered by inspecting the binary rather
/// than by reading the io `ErrorKind`, because the same two kinds are returned
/// when `current_dir` is what failed.
#[cfg(unix)]
#[test]
fn only_permanent_spawn_failures_claim_the_setup_marker() {
    use std::os::unix::fs::PermissionsExt;
    const MARKER: &str = "[claude-code] `claude` CLI";

    let dir = tempfile::tempdir().expect("tempdir");
    let healthy = dir.path().join("claude");
    std::fs::write(&healthy, "#!/bin/sh\n").expect("write bin");
    std::fs::set_permissions(&healthy, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    // Whatever the errno, an installed and executable CLI is not a broken
    // install — the failure is transient or environmental, and stays retryable.
    for detail in [
        "Resource busy (os error 16)",
        "Resource temporarily unavailable",
    ] {
        let err = super::spawn_error(&healthy, &detail);
        assert!(
            !err.to_string().contains(MARKER),
            "a healthy binary must stay retryable: {detail}"
        );
    }

    let err = super::spawn_error(&dir.path().join("gone"), &"boom");
    assert!(
        err.to_string().contains(MARKER),
        "an absent binary is a broken install and must classify as provider_setup"
    );
}

/// Under the Seatbelt jail the spawned program is `/usr/bin/sandbox-exec`, not
/// the CLI, so a missing or non-executable `claude` starts fine and fails on
/// exit instead. `spawn_error` never sees that path, and without this check the
/// user is told to report a broken install on Discord.
#[test]
fn a_missing_cli_is_a_setup_failure_even_when_the_spawn_itself_succeeded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let absent = dir.path().join("claude");

    let detail = unusable(super::probe_cli(&absent)).expect("an absent binary is a setup failure");
    assert!(
        detail.starts_with("[claude-code] `claude` CLI"),
        "detail must carry the setup marker: {detail}"
    );
}

/// The setup detail of a probe, or `None` for any other outcome — the shape the
/// per-kind assertions below are written against.
fn unusable(probe: super::CliProbe) -> Option<String> {
    match probe {
        super::CliProbe::Unusable(detail) => Some(detail),
        _ => None,
    }
}

#[cfg(unix)]
#[test]
fn a_cli_that_lost_its_execute_bit_is_a_setup_failure() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let bin = dir.path().join("claude");
    std::fs::write(&bin, "#!/bin/sh\n").expect("write bin");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o644)).expect("chmod");

    let detail =
        unusable(super::probe_cli(&bin)).expect("a non-executable binary is a setup failure");
    assert!(detail.contains("not executable"), "{detail}");
}

/// The inverse, and the one that keeps this from swallowing real turn failures:
/// a healthy binary that exits non-zero is a turn problem, not an install one.
#[cfg(unix)]
#[test]
fn a_healthy_cli_is_not_reported_as_a_setup_failure() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let bin = dir.path().join("claude");
    std::fs::write(&bin, "#!/bin/sh\nexit 1\n").expect("write bin");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    assert!(matches!(super::probe_cli(&bin), super::CliProbe::Healthy));
}

/// K1: every `stat` errno used to collapse into "is no longer present" and
/// reach the user as a NON-RETRYABLE `provider_setup` failure. Two of them are
/// install problems and must say which; every other one is not, and must not
/// permanently mark the provider broken.
///
/// The classification is asserted through the real classifier
/// (`web_chat::classify_inference_error`) rather than by eyeballing the string,
/// because the marker prefix is the whole contract between these two modules.
#[test]
fn a_stat_failure_is_classified_by_kind_not_collapsed_into_absent() {
    use crate::openhuman::web_chat::classify_inference_error;
    use std::io::{Error, ErrorKind};

    let bin = std::path::Path::new("/opt/tools/claude");

    // NotFound: the binary really is gone. Setup failure, non-retryable.
    let detail = unusable(super::metadata_failure(
        bin,
        &Error::from(ErrorKind::NotFound),
    ))
    .expect("NotFound is a setup failure");
    assert!(
        detail.contains("is no longer present"),
        "NotFound must say the binary is absent: {detail}"
    );
    let classified = classify_inference_error(&detail);
    assert_eq!(classified.error_type, "provider_setup");
    assert!(!classified.retryable);

    // PermissionDenied: also a setup failure, but the file may be fine and the
    // fix is the permissions on the path — so it must NOT claim absence.
    let detail = unusable(super::metadata_failure(
        bin,
        &Error::from(ErrorKind::PermissionDenied),
    ))
    .expect("PermissionDenied is a setup failure");
    assert!(
        detail.contains("permission denied"),
        "PermissionDenied must name the permission problem: {detail}"
    );
    assert!(
        !detail.contains("is no longer present"),
        "a readable-but-unreadable binary is not absent: {detail}"
    );
    let classified = classify_inference_error(&detail);
    assert_eq!(classified.error_type, "provider_setup");
    assert!(!classified.retryable);

    // Any other kind — EIO on a network mount, ELOOP, ENAMETOOLONG — says
    // nothing about the install. The io text is carried, the setup marker is
    // not, and the turn stays retryable.
    let probe = super::metadata_failure(bin, &Error::new(ErrorKind::Other, "input/output error"));
    let reason = match probe {
        super::CliProbe::Indeterminate(reason) => reason,
        super::CliProbe::Unusable(detail) => {
            panic!("a transient io error must not mark the install broken: {detail}")
        }
        super::CliProbe::Healthy => panic!("a failed stat is not a healthy binary"),
    };
    assert!(
        reason.contains("input/output error"),
        "the io error text must survive: {reason}"
    );

    let err = super::spawn_error_for_probe(probe_other(), &"Resource busy (os error 16)");
    let text = err.to_string();
    assert!(
        text.contains("input/output error") && text.contains("Resource busy"),
        "both the spawn errno and the probe errno must be carried: {text}"
    );
    let classified = classify_inference_error(&text);
    assert_ne!(
        classified.error_type, "provider_setup",
        "a transient io error must not classify as a setup failure: {text}"
    );
}

/// A fresh `Indeterminate` probe, since `CliProbe` is moved when inspected.
fn probe_other() -> super::CliProbe {
    super::metadata_failure(
        std::path::Path::new("/opt/tools/claude"),
        &std::io::Error::new(std::io::ErrorKind::Other, "input/output error"),
    )
}

/// A spawn failure caused by the working directory must not be blamed on the
/// CLI. `spawn` returns `NotFound`/`PermissionDenied` for an unusable
/// `current_dir` exactly as it does for a missing binary, so classifying by
/// `ErrorKind` alone told users to reinstall a CLI that was fine — and did it
/// non-retryably.
#[cfg(unix)]
#[test]
fn a_working_directory_failure_is_not_blamed_on_the_cli() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let bin = dir.path().join("claude");
    std::fs::write(&bin, "#!/bin/sh\n").expect("write bin");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let err = super::spawn_error(&bin, &"No such file or directory (os error 2)");
    let text = err.to_string();
    assert!(
        !text.contains("[claude-code] `claude` CLI"),
        "a healthy binary must not carry the setup marker: {text}"
    );
}

#[test]
fn a_spawn_failure_on_an_absent_cli_still_carries_the_setup_marker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = super::spawn_error(&dir.path().join("claude"), &"os error 2");
    assert!(err.to_string().starts_with("[claude-code] `claude` CLI"));
}

#[tokio::test]
async fn external_channel_turns_are_refused() {
    use crate::openhuman::agent::turn_origin::{self, AgentTurnOrigin};

    let err = turn_origin::with_origin(
        AgentTurnOrigin::ExternalChannel {
            channel: "discord".into(),
            sender: Some("attacker#1234".into()),
            reply_target: "chan-1".into(),
            message_id: "msg-1".into(),
        },
        async { super::refuse_untrusted_origin() },
    )
    .await
    .expect_err("an external-channel turn must not reach the CLI");
    assert!(
        err.to_string().contains("external channels"),
        "unexpected error: {err}"
    );
}

#[test]
fn unlabelled_origin_is_refused() {
    let err = super::refuse_untrusted_origin()
        .expect_err("an unlabelled origin must fail closed, not be assumed local");
    assert!(
        err.to_string().contains("labelled turn origin"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn web_chat_turns_are_allowed() {
    use crate::openhuman::agent::turn_origin::{self, AgentTurnOrigin};

    turn_origin::with_origin(
        AgentTurnOrigin::WebChat {
            thread_id: "t1".into(),
            client_id: "c1".into(),
            request_id: None,
        },
        async { super::refuse_untrusted_origin() },
    )
    .await
    .expect("the desktop user's own chat is exactly what this provider is for");
}

/// A directory has execute bits, and they mean "traversable", not "runnable".
/// Answering `Healthy` for one sends the EACCES from `spawn` down the retryable
/// generic path, which tells the user to report a broken install as a bug.
#[test]
fn a_directory_is_not_a_usable_cli() {
    let dir = tempfile::tempdir().expect("tempdir");
    let detail = unusable(super::probe_cli(dir.path()))
        .expect("a directory is a setup failure, not a turn failure");
    assert!(
        detail.contains("is not a regular file"),
        "unexpected detail: {detail}"
    );
}
