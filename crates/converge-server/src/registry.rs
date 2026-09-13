//! Maps document ids to live actors; spawns on first use.

use std::sync::Arc;

use converge_proto::DocId;
use converge_storage::Storage;
use dashmap::DashMap;
use tokio::sync::Mutex;

use crate::actor::{self, DocHandle};
use crate::config::Config;

#[derive(Clone)]
pub struct Registry {
    docs: Arc<DashMap<DocId, DocHandle>>,
    spawn_lock: Arc<Mutex<()>>,
    cfg: Config,
    storage: Arc<dyn Storage>,
}

impl Registry {
    pub fn new(cfg: Config, storage: Arc<dyn Storage>) -> Self {
        Registry {
            docs: Arc::default(),
            spawn_lock: Arc::default(),
            cfg,
            storage,
        }
    }

    pub fn storage(&self) -> &Arc<dyn Storage> {
        &self.storage
    }

    pub fn get(&self, doc: &DocId) -> Option<DocHandle> {
        self.docs.get(doc).map(|h| h.clone())
    }

    /// The actor for `doc`, loading it if necessary. Serialised so two
    /// concurrent first opens cannot spawn two actors.
    pub async fn get_or_spawn(&self, doc: &DocId) -> anyhow::Result<DocHandle> {
        if let Some(h) = self.get(doc) {
            if !h.tx.is_closed() {
                return Ok(h);
            }
        }
        let _g = self.spawn_lock.lock().await;
        if let Some(h) = self.get(doc) {
            if !h.tx.is_closed() {
                return Ok(h);
            }
        }
        let docs = self.docs.clone();
        let key = doc.clone();
        let handle = actor::spawn(
            doc.clone(),
            self.cfg.clone(),
            self.storage.clone(),
            move || {
                docs.remove_if(&key, |_, h| h.tx.is_closed());
            },
        )
        .await?;
        self.docs.insert(doc.clone(), handle.clone());
        Ok(handle)
    }

    pub fn open_docs(&self) -> Vec<DocId> {
        self.docs
            .iter()
            .filter(|e| !e.value().tx.is_closed())
            .map(|e| e.key().clone())
            .collect()
    }

    pub async fn shutdown(&self) {
        for e in self.docs.iter() {
            let _ = e.value().tx.send(actor::DocCommand::Shutdown).await;
        }
    }
}
