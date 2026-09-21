mod open;
mod popup;
pub mod reuse;
pub mod update;

use serde_json::{json, Value};

pub use open::run_open;

pub const VIEWER_TITLE: &str = "Hunks";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Overlay,
    Popup,
    Split,
}

pub fn open_params(
    plugin_id: &str,
    placement: Placement,
    opener: Option<&str>,
    repo_cwd: &str,
) -> Value {
    let mut params = json!({
        "plugin_id": plugin_id,
        "entrypoint": "viewer",
        "placement": match placement { Placement::Overlay => "overlay", Placement::Popup => "popup", Placement::Split => "split" },
        "cwd": repo_cwd,
        "env": {},
        "focus": true,
    });
    if placement != Placement::Overlay {
        params["env"]["HERDR_HUNKS_PLACEMENT"] = params["placement"].clone();
    }
    if placement == Placement::Popup {
        params["width"] = json!("80%");
        params["height"] = json!("80%");
    }
    if let Some(opener) = opener {
        params["env"]["HERDR_HUNKS_OPENER_PANE"] = json!(opener);
        if placement == Placement::Split {
            params["target_pane_id"] = json!(opener);
        }
    }
    if placement == Placement::Split {
        params["direction"] = json!("right");
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_params_carry_no_target_or_direction() {
        let p = open_params("test.hunks", Placement::Overlay, Some("w1:p1"), "/repo");
        assert_eq!(p["placement"], "overlay");
        assert!(p.get("target_pane_id").is_none() && p.get("direction").is_none());
        assert_eq!(p["cwd"], "/repo");
        assert_eq!(p["env"]["HERDR_HUNKS_OPENER_PANE"], "w1:p1");
        assert_eq!(p["entrypoint"], "viewer");
        assert_eq!(p["focus"], true);
    }

    #[test]
    fn split_params_target_the_opener_to_the_right() {
        let p = open_params("test.hunks", Placement::Split, Some("w1:p1"), "/repo");
        assert_eq!(
            (
                p["placement"].as_str(),
                p["target_pane_id"].as_str(),
                p["direction"].as_str()
            ),
            (Some("split"), Some("w1:p1"), Some("right"))
        );
    }
}
