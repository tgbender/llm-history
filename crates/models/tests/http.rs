use anyhow::Result;
use llm_history_models::{Embedder, HttpEngine, InferenceError, Reranker, gemma_profile};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

fn server(
    count: usize,
    respond: impl Fn(&str, &Value) -> (u16, Value) + Send + 'static,
) -> Result<(String, std::thread::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let handle = std::thread::spawn(move || {
        for _ in 0..count {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                stream.read_exact(&mut b).unwrap();
                header.push(b[0]);
            }
            let header = String::from_utf8(header).unwrap();
            let path = header.split_whitespace().nth(1).unwrap();
            let length = header
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|s| s.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            let (status, data) = respond(path, &body);
            let body = data.to_string();
            write!(stream,"HTTP/1.1 {status} response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    Ok((endpoint, handle))
}
#[test]
fn embedding_prefix_indices_and_normalization() -> Result<()> {
    let (endpoint, handle) = server(3, |path, body| {
        if path == "/tokenize" {
            assert!(
                body["content"]
                    .as_str()
                    .unwrap()
                    .starts_with("title: none | text: ")
            );
            return (200, json!({"tokens":[2,3]}));
        }
        assert_eq!(path, "/v1/embeddings");
        assert_eq!(body["input"].as_array().unwrap().len(), 2);
        (
            200,
            json!({"data":[{"index":1,"embedding":[0.,4.]},{"index":0,"embedding":[3.,4.]}]}),
        )
    })?;
    let mut p = gemma_profile(768)?;
    p.dimensions = 2;
    let mut engine = HttpEngine::new(&endpoint, None, "test", p, Duration::from_secs(5))?;
    assert_eq!(
        engine.embed(&["one".into(), "two".into()], false)?,
        vec![vec![0.6, 0.8], vec![0., 1.]]
    );
    handle.join().unwrap();
    Ok(())
}
#[test]
fn reranker_validates_indices_and_scores() -> Result<()> {
    let (endpoint, handle) = server(1, |path, _| {
        assert_eq!(path, "/v1/rerank");
        (
            200,
            json!({"results":[{"index":0,"relevance_score":0.2},{"index":1,"relevance_score":0.9}]}),
        )
    })?;
    let mut engine = HttpEngine::new(
        &endpoint,
        None,
        "test",
        gemma_profile(768)?,
        Duration::from_secs(5),
    )?;
    assert_eq!(
        engine.rerank("query", &["a".into(), "b".into()])?,
        vec![(1, 0.9), (0, 0.2)]
    );
    handle.join().unwrap();
    Ok(())
}
#[test]
fn http_failures_have_sanitized_typed_status() -> Result<()> {
    let (endpoint, handle) = server(1, |_, _| {
        (503, json!({"error":"sensitive server response"}))
    })?;
    let mut engine = HttpEngine::new(
        &endpoint,
        None,
        "test",
        gemma_profile(768)?,
        Duration::from_secs(5),
    )?;
    let error = engine.rerank("query", &["a".into()]).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<InferenceError>(),
        Some(InferenceError::Http(503))
    ));
    assert!(!error.to_string().contains("sensitive"));
    handle.join().unwrap();
    Ok(())
}
