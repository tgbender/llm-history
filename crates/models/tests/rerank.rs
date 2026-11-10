use anyhow::Result;
use llm_history_models::{
    InferenceError, ModelProfile, Reranker, qwen_reranker_profile, rerank_resilient,
};
struct Limited {
    profile: ModelProfile,
    calls: usize,
}
impl Reranker for Limited {
    fn profile(&self) -> &ModelProfile {
        &self.profile
    }
    fn rerank(&mut self, query: &str, docs: &[String]) -> Result<Vec<(usize, f64)>> {
        self.calls += 1;
        if query == "oversized" || docs.len() > 2 || docs.iter().any(|d| d.chars().count() > 8) {
            return Err(InferenceError::TooLong.into());
        }
        Ok(docs
            .iter()
            .enumerate()
            .map(|(i, s)| (i, if s.contains('z') { 0.9 } else { 0.1 }))
            .collect())
    }
}
#[test]
fn oversized_batches_and_unicode_passages_keep_original_indexes() -> Result<()> {
    let mut model = Limited {
        profile: qwen_reranker_profile(),
        calls: 0,
    };
    let result = rerank_resilient(
        &mut model,
        "q",
        &["short".into(), "ééééééééééééééz".into(), "third".into()],
    )?;
    assert_eq!(result.scores[0], (1, 0.9));
    assert_eq!(result.scores.len(), 3);
    assert!(result.failed.is_empty());
    let result = rerank_resilient(&mut model, "oversized", &["a".into(), "b".into()])?;
    assert!(result.scores.is_empty());
    assert_eq!(result.failed, vec![0, 1]);
    model.calls = 0;
    let result = rerank_resilient(&mut model, "oversized", &["long passage ".repeat(1000)])?;
    assert!(result.scores.is_empty());
    assert_eq!(result.failed, vec![0]);
    assert!(model.calls <= 17, "{} requests", model.calls);
    Ok(())
}
