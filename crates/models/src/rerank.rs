//! Recover token-limit failures by shrinking only the affected request.
use super::*;

pub struct RerankResult {
    pub scores: Vec<(usize, f64)>,
    pub failed: Vec<usize>,
}
pub fn rerank_resilient(
    engine: &mut dyn Reranker,
    query: &str,
    documents: &[String],
) -> Result<RerankResult> {
    fn one(
        engine: &mut dyn Reranker,
        query: &str,
        text: &str,
        remaining: &mut usize,
    ) -> Result<Option<f64>> {
        if *remaining == 0 {
            return Ok(None);
        }
        *remaining -= 1;
        match engine.rerank(query, &[text.into()]) {
            Ok(scores) => {
                validate(&scores, 1)?;
                Ok(Some(scores[0].1))
            }
            Err(e)
                if matches!(
                    e.downcast_ref::<InferenceError>(),
                    Some(InferenceError::TooLong)
                ) =>
            {
                let Some((left, right)) = halves(text) else {
                    return Ok(None);
                };
                let a = one(engine, query, left, remaining)?;
                let b = one(engine, query, right, remaining)?;
                Ok(match (a, b) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                })
            }
            Err(e) => Err(e),
        }
    }
    fn batch(
        engine: &mut dyn Reranker,
        query: &str,
        docs: &[String],
        offset: usize,
        out: &mut RerankResult,
    ) -> Result<()> {
        if docs.is_empty() {
            return Ok(());
        }
        match engine.rerank(query, docs) {
            Ok(scores) => {
                validate(&scores, docs.len())?;
                out.scores
                    .extend(scores.into_iter().map(|(i, s)| (i + offset, s)));
            }
            Err(e)
                if matches!(
                    e.downcast_ref::<InferenceError>(),
                    Some(InferenceError::TooLong)
                ) =>
            {
                if docs.len() > 1 {
                    let n = docs.len() / 2;
                    batch(engine, query, &docs[..n], offset, out)?;
                    batch(engine, query, &docs[n..], offset + n, out)?;
                } else if let Some(score) = one(engine, query, &docs[0], &mut 16)? {
                    out.scores.push((offset, score));
                } else {
                    out.failed.push(offset);
                }
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }
    let mut result = RerankResult {
        scores: Vec::new(),
        failed: Vec::new(),
    };
    batch(engine, query, documents, 0, &mut result)?;
    result
        .scores
        .sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(result)
}
fn validate(scores: &[(usize, f64)], n: usize) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    if scores.len() != n
        || scores
            .iter()
            .any(|(i, s)| *i >= n || !s.is_finite() || !seen.insert(*i))
    {
        bail!("reranker contract violation");
    }
    Ok(())
}
fn halves(text: &str) -> Option<(&str, &str)> {
    let n = text.chars().count();
    if n < 2 {
        return None;
    }
    let mid = text.char_indices().nth(n / 2)?.0;
    let at = text[..mid]
        .rfind('\n')
        .filter(|i| *i > mid / 2)
        .map_or(mid, |i| i + 1);
    Some(text.split_at(at))
}
