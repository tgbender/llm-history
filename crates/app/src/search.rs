//! Application search workflow.
use crate::{
    History,
    api::search::{SearchHit, SearchResponse, SimilarHit, SimilarRequest},
};
use anyhow::Result;
use llm_history_models::{Embedder, Reranker};
use llm_history_types::query::SearchRequest;

impl History {
    pub fn similar(&self, request: &SimilarRequest) -> Result<Vec<SimilarHit>> {
        anyhow::ensure!(request.message_no > 0, "message ID must be positive");
        anyhow::ensure!(
            (1..=100).contains(&request.limit) && (1..=100).contains(&request.per_session),
            "limit and per-session must be 1..=100"
        );
        let model = llm_history_models::gemma_profile(request.dimensions)?;
        let rows = self.open(true)?.similar_messages(
            request.message_no,
            &model.id,
            &request.filter,
            request.limit,
            request.per_session,
            request.other_sessions,
        )?;
        let budget = (5000 / request.limit).min(1000);
        Ok(rows
            .into_iter()
            .map(|(mut message, similarity)| {
                message.text = llm_history_search::passage(&message.text, "", budget, true, false);
                SimilarHit {
                    message: message.into(),
                    similarity,
                }
            })
            .collect())
    }

    /// Search retained history only. Refresh and background work are explicit operations.
    pub fn search(
        &self,
        request: &SearchRequest,
        embedder: Option<&mut dyn Embedder>,
        reranker: Option<&mut dyn Reranker>,
    ) -> Result<SearchResponse> {
        let response = llm_history_search::search(|| self.open(true), request, embedder, reranker)?;
        Ok(SearchResponse {
            results: response
                .results
                .into_iter()
                .map(|hit| SearchHit {
                    message: hit.message.into(),
                    score: hit.score,
                    lexical_score: hit.lexical_score,
                    semantic_score: hit.semantic_score,
                    rerank_score: hit.rerank_score,
                    verdict: hit.verdict,
                })
                .collect(),
            matched: response.matched,
            details: response.details,
        })
    }
}
