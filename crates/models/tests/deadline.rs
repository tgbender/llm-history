use anyhow::Result;
use llm_history_models::{HttpEngine, InferenceError, TokenCounter, gemma_profile};
use std::{
    io::Read,
    net::TcpListener,
    time::{Duration, Instant},
};

#[test]
fn worker_deadline_bounds_a_tokenizer_request_below_the_client_timeout() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut byte = [0];
        let _ = stream.read(&mut byte);
        std::thread::sleep(Duration::from_millis(250));
    });
    let mut engine = HttpEngine::new(
        &endpoint,
        None,
        "test",
        gemma_profile(768)?,
        Duration::from_secs(10),
    )?;
    let started = Instant::now();
    engine.set_deadline(started + Duration::from_millis(100));
    let error = engine.token_count("test").unwrap_err();
    assert!(matches!(
        error.downcast_ref::<InferenceError>(),
        Some(InferenceError::Transport)
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
    thread.join().unwrap();
    Ok(())
}
