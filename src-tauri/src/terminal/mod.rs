use std::{collections::HashMap, sync::Mutex};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::SessionProtocol;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Idle,
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRuntime {
    pub runtime_id: String,
    pub profile_id: Option<String>,
    pub kind: SessionProtocol,
    pub status: RuntimeStatus,
    pub pane_id: String,
    pub tab_id: String,
    pub title: String,
}

#[derive(Default)]
pub struct RuntimeRegistry {
    runtimes: Mutex<HashMap<String, TerminalRuntime>>,
}

impl RuntimeRegistry {
    pub fn open_local(&self, tab_id: String, pane_id: String) -> Result<TerminalRuntime, String> {
        let runtime = TerminalRuntime {
            runtime_id: Uuid::new_v4().to_string(),
            profile_id: None,
            kind: SessionProtocol::Local,
            status: RuntimeStatus::Connected,
            pane_id,
            tab_id,
            title: "Local Shell".to_string(),
        };
        self.runtimes
            .lock()
            .map_err(|_| "terminal runtime registry lock poisoned".to_string())?
            .insert(runtime.runtime_id.clone(), runtime.clone());
        Ok(runtime)
    }

    pub fn open_ssh_placeholder(
        &self,
        tab_id: String,
        pane_id: String,
        title: String,
    ) -> Result<TerminalRuntime, String> {
        let runtime = TerminalRuntime {
            runtime_id: Uuid::new_v4().to_string(),
            profile_id: None,
            kind: SessionProtocol::Ssh,
            status: RuntimeStatus::Connecting,
            pane_id,
            tab_id,
            title,
        };
        self.runtimes
            .lock()
            .map_err(|_| "terminal runtime registry lock poisoned".to_string())?
            .insert(runtime.runtime_id.clone(), runtime.clone());
        Ok(runtime)
    }

    pub fn close(&self, runtime_id: &str) -> Result<(), String> {
        self.runtimes
            .lock()
            .map_err(|_| "terminal runtime registry lock poisoned".to_string())?
            .remove(runtime_id);
        Ok(())
    }
}
