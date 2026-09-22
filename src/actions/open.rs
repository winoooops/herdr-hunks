use std::process::Command;

use serde_json::Value;

use super::{open_params, reuse, Placement, VIEWER_TITLE};
use crate::herdr::client::{HerdrClient, HerdrClientError};
use crate::text::sanitize;

pub fn run_open(placement: Placement) -> i32 {
    match run(placement) {
        Ok(()) => 0,
        Err(error) => {
            let message = error
                .downcast_ref::<HerdrClientError>()
                .and_then(|error| {
                    if let HerdrClientError::Api(body) = error {
                        serde_json::from_str::<Value>(body).ok()?["message"]
                            .as_str()
                            .map(str::to_owned)
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| error.to_string());
            eprintln!("herdr-hunks: {}", sanitize(&message));
            1
        }
    }
}

fn run(placement: Placement) -> Result<(), Box<dyn std::error::Error>> {
    let plugin_id = std::env::var("HERDR_PLUGIN_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .ok_or("HERDR_PLUGIN_ID is not set (run this through a herdr plugin action)")?;
    let context: Value = serde_json::from_str(
        &std::env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_else(|_| "{}".into()),
    )?;
    let opener = context["focused_pane_id"].as_str();
    let client = HerdrClient::from_env();
    let pane = opener.map(|id| client.pane_get(id)).transpose()?;
    if pane.as_ref().and_then(|p| p.label.as_deref()) == Some(VIEWER_TITLE) {
        eprintln!("herdr-hunks: already in the hunk viewer");
        return Ok(());
    }
    let repo_cwd = pane
        .as_ref()
        .and_then(|p| p.foreground_cwd.as_deref().or(p.cwd.as_deref()))
        .or_else(|| context["focused_pane_cwd"].as_str())
        .or_else(|| context["workspace_cwd"].as_str())
        .ok_or("no working directory for the focused pane")?;
    let mut params = open_params(&plugin_id, placement, opener, repo_cwd);
    if placement == Placement::Popup {
        if let Some(dir) = crate::paths::config_dir(|key| std::env::var_os(key)) {
            let ([width, height], problems) = super::popup::load(&dir);
            params["width"] = width;
            params["height"] = height;
            for problem in problems {
                eprintln!("herdr-hunks: {}", sanitize(&problem));
            }
        }
    }
    if placement == Placement::Split {
        let state_dir = crate::paths::state_dir(|key| std::env::var_os(key));
        let socket = std::env::var("HERDR_SOCKET_PATH").ok();
        if let (Some(state_dir), Some(socket), Some(opener)) = (state_dir, socket, opener) {
            match reuse::with_lock(&state_dir, || -> Result<(), Box<dyn std::error::Error>> {
                let toplevel = Command::new("git")
                    .args(["-C", repo_cwd, "rev-parse", "--show-toplevel"])
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .and_then(|output| String::from_utf8(output.stdout).ok())
                    .map(|path| path.trim().to_owned())
                    .filter(|path| !path.is_empty())
                    .unwrap_or_else(|| repo_cwd.to_owned());
                let key = reuse::key(&socket, opener);
                let mut records = reuse::load(&state_dir);
                if let Some(record) = records.get(&key) {
                    let viewer = client.pane_get(&record.viewer_pane_id).ok();
                    if reuse::may_reuse(record, &toplevel, viewer.as_ref())
                        && client.plugin_pane_focus(&record.viewer_pane_id).is_ok()
                    {
                        return Ok(());
                    }
                }
                let viewer_pane_id = client.plugin_pane_open(params.clone())?;
                records.remove(&key);
                if let Some(viewer_pane_id) = viewer_pane_id {
                    records.insert(
                        key,
                        reuse::Record {
                            viewer_pane_id,
                            repo_cwd: repo_cwd.to_owned(),
                            toplevel,
                        },
                    );
                }
                if let Err(error) = reuse::save(&state_dir, &records) {
                    eprintln!(
                        "herdr-hunks: could not save split reuse record: {}",
                        sanitize(&error.to_string())
                    );
                }
                Ok(())
            }) {
                Ok(result) => return result,
                Err(error) => eprintln!(
                    "herdr-hunks: split reuse unavailable: {}; opening without reuse",
                    sanitize(&error.to_string())
                ),
            }
        } else {
            eprintln!("herdr-hunks: split reuse unavailable (no absolute state directory, socket path, or opener); opening without reuse");
        }
    }
    client.plugin_pane_open(params)?;
    Ok(())
}
