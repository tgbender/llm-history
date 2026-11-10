#![cfg(unix)]
use anyhow::Result;
use llm_history_models::{CancellationToken, ManagedLlama, gemma_profile};
use std::{
    os::unix::fs::PermissionsExt,
    process::Command,
    time::{Duration, Instant},
};

fn fixture() -> Result<(tempfile::TempDir, std::path::PathBuf, std::path::PathBuf)> {
    let d = tempfile::tempdir()?;
    let script = d.path().join("server");
    let model = d.path().join("model.gguf");
    std::fs::write(&model, [])?;
    std::fs::write(&script, "#!/bin/sh\necho $$ > pid\nexec sleep 30\n")?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))?;
    Ok((d, script, model))
}
#[test]
fn startup_timeout_reaps_the_owned_child() -> Result<()> {
    let (d, binary, model) = fixture()?;
    let result = ManagedLlama::start(
        &binary,
        &model,
        false,
        gemma_profile(768)?,
        Duration::from_secs(1),
        CancellationToken::default(),
    );
    let error = result.err().expect("fake server cannot become healthy");
    assert!(error.to_string().contains("deadline"), "{error:#}");
    let pid = std::fs::read_to_string(d.path().join("pid"))?;
    assert!(
        !Command::new("kill")
            .args(["-0", pid.trim()])
            .output()?
            .status
            .success()
    );
    Ok(())
}
#[test]
fn startup_exit_reports_the_redacted_log_tail() -> Result<()> {
    let (_d, binary, model) = fixture()?;
    std::fs::write(
        &binary,
        "#!/bin/sh\necho \"args: $*\"\necho 'error: missing libggml' >&2\nexit 1\n",
    )?;
    let error = ManagedLlama::start(
        &binary,
        &model,
        false,
        gemma_profile(768)?,
        Duration::from_secs(5),
        CancellationToken::default(),
    )
    .err()
    .expect("fake server exits");
    let message = format!("{error:#}");
    assert!(message.contains("exited during startup"), "{message}");
    assert!(message.contains("error: missing libggml"), "{message}");
    assert!(message.contains("--api-key [REDACTED] "), "{message}");
    Ok(())
}
#[test]
fn cancellation_ends_startup_and_reaps_the_owned_child() -> Result<()> {
    let (d, binary, model) = fixture()?;
    let token = CancellationToken::default();
    let cancel = token.clone();
    let pid_path = d.path().join("pid");
    let thread = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pid_path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        cancel.cancel();
    });
    let start = Instant::now();
    assert!(
        ManagedLlama::start(
            &binary,
            &model,
            false,
            gemma_profile(768)?,
            Duration::from_secs(10),
            token
        )
        .is_err()
    );
    thread.join().unwrap();
    assert!(start.elapsed() < Duration::from_secs(7));
    let pid = std::fs::read_to_string(d.path().join("pid"))?;
    assert!(
        !Command::new("kill")
            .args(["-0", pid.trim()])
            .output()?
            .status
            .success()
    );
    Ok(())
}
