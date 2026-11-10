//! Search-time refresh. Unchanged inventories stay on the read path.
use crate::History;
use anyhow::Result;
use llm_history_ingest::{PARSER_VERSION, catalog::catalog_fingerprint, source::discover};
use llm_history_store::Store;
use llm_history_types::{Harness, Profile};
use llm_history_types::{ingestion::UpdateReceipt, storage::DatabaseBusy};
use std::{collections::HashMap, time::Duration};

const FAILURE_BACKOFF_SECONDS: i64 = 300;

impl History {
    pub fn refresh(
        &self,
        names: &[String],
        source: Option<&str>,
        force: bool,
    ) -> Result<Vec<UpdateReceipt>> {
        if let Some(source) = source.filter(|s| *s != "all") {
            let _: Harness = source.parse()?;
        }
        if self.database().is_file() && !force {
            let s = self.open(true)?;
            if !s.profiles(&[], false)?.is_empty()
                && refresh_profiles(&s, names, source, false)?.is_empty()
            {
                return Ok(Vec::new());
            }
        }
        // Automatic refresh can search the committed archive while another update prepares.
        let _ingestion = if force {
            self.lock_ingestion()?
        } else {
            match Store::lock_ingestion(self.database(), Duration::ZERO) {
                Ok(lock) => lock,
                Err(error) if error.is::<DatabaseBusy>() => return Ok(Vec::new()),
                Err(error) => return Err(error),
            }
        };
        let selected = {
            let existing = self.database().is_file();
            let mut store = self.open(existing)?;
            if !existing || store.profiles(&[], false)?.is_empty() {
                if existing {
                    drop(store);
                    store = self.open(false)?;
                }
                self.defaults(&store)?;
            }
            // Another caller may have refreshed since our first inventory check.
            refresh_profiles(&store, names, source, force)?
        };
        let mut receipts = Vec::new();
        for (p, had_failure) in selected {
            let receipt = crate::ingestion::update(self, std::slice::from_ref(&p.name), false)?;
            let failure = (!receipt.errors.is_empty()).then(|| chrono::Utc::now().timestamp());
            if failure.is_some() || had_failure {
                self.open(false)?.set_refresh_failure(&p.id, failure)?;
            }
            receipts.push(receipt);
        }
        Ok(receipts)
    }
}
fn refresh_profiles(
    s: &Store,
    names: &[String],
    source: Option<&str>,
    force: bool,
) -> Result<Vec<(Profile, bool)>> {
    let now = chrono::Utc::now().timestamp();
    let mut selected = Vec::new();
    for p in s.profiles(names, true)? {
        if source.is_some_and(|source| source != "all" && source != p.harness.key()) {
            continue;
        }
        let failed_at = s.refresh_failure_at(&p.id);
        if !force && failed_at.is_some_and(|at| (0..FAILURE_BACKOFF_SECONDS).contains(&(now - at)))
        {
            continue;
        }
        if force || has_changes(s, &p)? {
            selected.push((p, failed_at.is_some()));
        }
    }
    Ok(selected)
}

fn has_changes(store: &Store, profile: &Profile) -> Result<bool> {
    if store.refresh_projection_pending(&profile.id)? {
        return Ok(true);
    }
    let mut stamps = HashMap::new();
    for stamp in store.source_stamps(profile)? {
        if stamp.decoder_revision != PARSER_VERSION {
            return Ok(true);
        }
        stamps.insert(stamp.key.clone(), stamp);
    }
    // Let normal ingestion report filesystem errors and apply its recovery rules.
    let Ok(sources) = discover(profile) else {
        return Ok(true);
    };
    for source in sources {
        let Some(prior) = stamps.remove(&source.key) else {
            return Ok(true);
        };
        let Ok(observed) = source.observe() else {
            return Ok(true);
        };
        if prior.missing
            || prior.uri != source.path.to_string_lossy()
            || !observed.matches(prior.size, prior.mtime_ns, &prior.source_token)
        {
            return Ok(true);
        }
    }
    if stamps.values().any(|stamp| !stamp.missing) {
        return Ok(true);
    }
    let Ok(current) = catalog_fingerprint(profile) else {
        return Ok(true);
    };
    Ok(store.catalog_revision(profile)?.as_deref() != Some(current.as_str()))
}
