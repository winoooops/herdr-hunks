use serde::Deserialize;
use serde_json::{json, Value};

use super::client::{HerdrClient, HerdrClientError};

#[derive(Debug, Deserialize)]
pub struct PaneInfo {
    pub pane_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
}

impl HerdrClient {
    pub fn pane_get(&self, pane_id: &str) -> Result<PaneInfo, HerdrClientError> {
        let response = self.request("pane.get", json!({ "pane_id": pane_id }))?;
        Ok(serde_json::from_value(response["result"]["pane"].clone())?)
    }

    pub fn plugin_pane_open(&self, params: Value) -> Result<Option<String>, HerdrClientError> {
        let response = self.request("plugin.pane.open", params)?;
        Ok(response["result"]["plugin_pane"]["pane"]["pane_id"]
            .as_str()
            .map(str::to_owned))
    }

    pub fn plugin_pane_focus(&self, pane_id: &str) -> Result<(), HerdrClientError> {
        self.request("plugin.pane.focus", json!({ "pane_id": pane_id }))?;
        Ok(())
    }
}
