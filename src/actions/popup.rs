use serde_json::{json, Value};
use std::path::Path;

pub fn load(dir: &Path) -> ([Value; 2], Vec<String>) {
    let mut sizes = [json!("80%"), json!("80%")];
    let mut problems = Vec::new();
    let text = match std::fs::read_to_string(dir.join("config.toml")) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                problems.push(format!("config.toml: {error}"));
            }
            return (sizes, problems);
        }
    };
    let table: toml::Table = match text.parse() {
        Ok(table) => table,
        Err(error) => return (sizes, vec![format!("config.toml: {error}")]),
    };
    let Some(popup) = table.get("popup") else {
        return (sizes, problems);
    };
    let Some(popup) = popup.as_table() else {
        return (sizes, vec!["popup: expected a table".into()]);
    };
    for (i, key) in ["width", "height"].iter().enumerate() {
        let Some(value) = popup.get(*key) else {
            continue;
        };
        match value {
            // The host takes 0..=65535 cells, or a percentage without a leading zero.
            toml::Value::Integer(n) if (20..=65_535).contains(n) => sizes[i] = json!(n),
            toml::Value::String(s)
                if s.strip_suffix('%').is_some_and(|n| {
                    !n.starts_with('0')
                        && n.bytes().all(|b| b.is_ascii_digit())
                        && n.parse::<u8>().is_ok_and(|n| (20..=100).contains(&n))
                }) =>
            {
                sizes[i] = json!(s)
            }
            _ => problems.push(format!(
                "popup.{key}: expected an integer 20..=65535 or \"20%\"..\"100%\"; using \"80%\""
            )),
        }
    }
    (sizes, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_sizes_validate_each_key_and_keep_the_valid_neighbor() {
        let dir = tempfile::tempdir().unwrap();
        for (value, expected, errors) in [
            ("20", json!(20), 0),
            ("2000", json!(2000), 0),
            ("\"20%\"", json!("20%"), 0),
            ("\"100%\"", json!("100%"), 0),
            ("19", json!("80%"), 1),
            ("-1", json!("80%"), 1),
            ("20.0", json!("80%"), 1),
            ("true", json!("80%"), 1),
            ("[]", json!("80%"), 1),
            ("{}", json!("80%"), 1),
            ("\"19%\"", json!("80%"), 1),
            ("\"101%\"", json!("80%"), 1),
            ("65535", json!(65535), 0),
            ("65536", json!("80%"), 1),
            ("\"080%\"", json!("80%"), 1),
            ("\"+50%\"", json!("80%"), 1),
            ("\" 50%\"", json!("80%"), 1),
            ("\"50\"", json!("80%"), 1),
        ] {
            for key in ["width", "height"] {
                let other = if key == "width" { "height" } else { "width" };
                std::fs::write(
                    dir.path().join("config.toml"),
                    format!("[popup]\n{key}={value}\n{other}=24\n"),
                )
                .unwrap();
                let (sizes, problems) = load(dir.path());
                let i = usize::from(key == "height");
                assert_eq!(sizes[i], expected, "{key}={value}");
                assert_eq!(sizes[1 - i], json!(24));
                assert_eq!(problems.len(), errors);
                if errors > 0 {
                    assert!(problems[0].contains(&format!("popup.{key}")));
                }
            }
        }
    }

    #[test]
    fn popup_missing_keys_default_and_broken_files_report_problems() {
        let dir = tempfile::tempdir().unwrap();
        let defaults = [json!("80%"), json!("80%")];
        assert_eq!(load(dir.path()), (defaults.clone(), vec![]));
        let path = dir.path().join("config.toml");
        for (text, errors) in [("", 0), ("[popup]", 0), ("popup=true", 1), ("[popup", 1)] {
            std::fs::write(&path, text).unwrap();
            let (sizes, problems) = load(dir.path());
            assert_eq!(sizes, defaults);
            assert_eq!(problems.len(), errors, "{text}");
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert_eq!(load(dir.path()).1.len(), 1);
    }
}
