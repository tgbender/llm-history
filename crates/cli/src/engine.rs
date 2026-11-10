//! Embedding and reranking engines: a managed local llama.cpp server or an endpoint.
use super::*;
use llm_history_models::{Embedder, HttpEngine, ManagedLlama, Reranker, gemma_profile};

pub(crate) fn inference_queue(db: &Path, inference: &Inference) -> Result<PathBuf> {
    Ok(db.with_extension(format!("{}.queue", profile_for(inference)?.id)))
}
fn key(path: &Option<PathBuf>) -> Result<Option<String>> {
    path.as_ref()
        .map(|p| Ok(std::fs::read_to_string(p)?.trim().to_owned()))
        .transpose()
}
pub(crate) fn profile_for(inference: &Inference) -> Result<llm_history_models::ModelProfile> {
    let mut p = gemma_profile(inference.dimensions)?;
    if let Some(model) = &inference.model_name {
        p.id = format!(
            "remote-{}-{}",
            model.replace(|c: char| !c.is_alphanumeric(), "-"),
            inference.dimensions
        );
        p.model = model.clone();
        p.engine_contract = "explicit-remote-v1".into();
    }
    Ok(p)
}
pub(crate) enum Engine {
    Managed(Box<ManagedLlama>),
    Remote(Box<HttpEngine>),
}
impl Engine {
    pub(crate) fn embedding(&mut self) -> &mut dyn Embedder {
        match self {
            Self::Managed(s) => &mut s.engine,
            Self::Remote(s) => s.as_mut(),
        }
    }
    pub(crate) fn reranking(&mut self) -> &mut dyn Reranker {
        match self {
            Self::Managed(s) => &mut s.engine,
            Self::Remote(s) => s.as_mut(),
        }
    }
}
pub(crate) fn engine(cache: &Path, args: &Inference, rerank: bool) -> Result<Engine> {
    let mut profile = if rerank {
        llm_history_models::qwen_reranker_profile()
    } else {
        profile_for(args)?
    };
    if rerank && let Some(model) = &args.rerank_model_name {
        profile.model = model.clone();
        profile.engine_contract = "explicit-remote-rerank-v1".into();
    }
    let deadline = Duration::from_secs(args.timeout_seconds);
    let endpoint = if rerank {
        &args.rerank_endpoint
    } else {
        &args.endpoint
    };
    if let Some(endpoint) = endpoint {
        let model = if rerank {
            args.rerank_model_name.as_deref().unwrap_or("reranker")
        } else {
            args.model_name.as_deref().unwrap_or("embeddinggemma-2")
        };
        let mut engine = HttpEngine::new(
            endpoint,
            key(if rerank {
                &args.rerank_api_key_file
            } else {
                &args.api_key_file
            })?,
            model,
            profile,
            deadline,
        )?;
        if !rerank {
            let artifact = &llm_history_models::model_artifacts()[2];
            if let Ok(path) = llm_history_models::artifact_path(artifact, false) {
                engine = engine.with_tokenizer(&path)?;
            }
        }
        return Ok(Engine::Remote(Box::new(engine)));
    }
    let binary = llm_history_models::native_server(cache)?;
    let model = llm_history_models::artifact_path(
        &llm_history_models::model_artifacts()[usize::from(rerank)],
        false,
    )?;
    let mut managed =
        ManagedLlama::start(&binary, &model, rerank, profile, deadline, cancellation())?;
    if !rerank {
        let artifact = &llm_history_models::model_artifacts()[2];
        if let Ok(path) = llm_history_models::artifact_path(artifact, false) {
            managed.engine.load_tokenizer(&path)?;
        }
    }
    Ok(Engine::Managed(Box::new(managed)))
}
