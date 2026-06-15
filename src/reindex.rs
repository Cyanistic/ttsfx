use crate::cache::{audio_paths_in_dir, CacheIndex, write_metadata};
use crate::cli::ReindexEmbeddingsArgs;
use crate::config::Config;
use crate::embed::embed_text;
use crate::error::ResultExt;
use crate::{Result, err};
use chrono::{DateTime, Utc};
use reqwest::Client;

use std::time::Duration;
use tracing::info;

pub async fn reindex_embeddings(config: &Config, args: &ReindexEmbeddingsArgs) -> Result<()> {
    let cache_dir = &config.overridable.cache_dir;
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| err!(Internal, "failed to build HTTP client", @external: e))?;

    let before_dt = args.before;
    let after_dt = args.after;

    let paths = audio_paths_in_dir(cache_dir);
    let mut candidates = 0usize;
    let mut updated = 0usize;

    for path in paths {
        let entry = match CacheIndex::load_entry(&path).await.warn() {
            Ok(e) => e,
            Err(_) => continue,
        };

        let recipe = if !entry.meta.recipe.is_empty() {
            entry.meta.recipe.clone()
        } else if args.force {
            entry.meta.matched_text.clone()
        } else {
            continue;
        };

        if recipe.is_empty() {
            continue;
        }

        if !is_candidate(&entry.meta.embedded_at, args.force, &before_dt, &after_dt) {
            continue;
        }

        candidates += 1;
        if args.dry_run {
            info!(path = %path.display(), recipe = %recipe, "reindex candidate");
            continue;
        }

        let vector = embed_text(&client, &config.overridable, &recipe).await?;
        let mut meta = entry.meta;
        meta.embed_model = Some(config.overridable.embed_model.clone());
        meta.embedded_at = Some(Utc::now());
        meta.embedding = Some(crate::cache::Embeddings::from(vector));
        write_metadata(&path, &meta).await?;
        updated += 1;
        info!(path = %path.display(), "reindexed embedding");
    }

    info!(
        candidates,
        updated,
        dry_run = args.dry_run,
        "reindex-embeddings finished"
    );
    Ok(())
}

fn is_candidate(
    embedded_at: &Option<DateTime<Utc>>,
    force: bool,
    before: &Option<DateTime<Utc>>,
    after: &Option<DateTime<Utc>>,
) -> bool {
    if force {
        return true;
    }
    if before.is_none() && after.is_none() {
        return embedded_at.is_none();
    }
    match embedded_at {
        None => true,
        Some(ts) => {
            if let Some(b) = before
                && *ts >= *b
            {
                return false;
            }
            if let Some(a) = after
                && *ts < *a
            {
                return false;
            }
            true
        }
    }
}

