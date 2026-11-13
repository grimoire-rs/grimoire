// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Reads the `GRIM_CONFIG` path list into parsed TOML tables.
//!
//! It only reads and parses; each consumer merges its own table.

use std::ffi::OsStr;
use std::path::PathBuf;

use crate::config::config_error::{ConfigError, ConfigErrorKind};

/// One parsed `GRIM_CONFIG` file.
pub struct Layer {
    pub path: PathBuf,
    pub table: toml::Table,
}

// Values can be header secrets, so only the top-level key names are printed.
impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer")
            .field("path", &self.path)
            .field("keys", &self.table.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// A `GRIM_CONFIG` entry could not be turned into a [`Layer`].
///
/// No variant carries file content: a TOML error quotes the offending
/// source line, which can hold a header secret.
#[derive(Debug, thiserror::Error)]
pub enum LayerError {
    #[error("GRIM_CONFIG entry '{}' is not an absolute path", entry.display())]
    Relative { entry: PathBuf },
    #[error("cannot read GRIM_CONFIG file '{}': {source}", path.display())]
    Read { path: PathBuf, source: std::io::Error },
    #[error("GRIM_CONFIG file '{}' is not a regular file", path.display())]
    NotRegular { path: PathBuf },
    #[error("GRIM_CONFIG file '{}' is larger than {limit} bytes", path.display())]
    TooLarge { path: PathBuf, limit: u64 },
    #[error("GRIM_CONFIG file '{}' is not valid TOML{}", path.display(), position_suffix(*position))]
    Parse {
        path: PathBuf,
        /// 1-based (line, column) of the first error.
        position: Option<(usize, usize)>,
    },
    /// For consumers: names the dotted key, never its value.
    #[error("GRIM_CONFIG file '{}' has an unusable value for '{key}'", path.display())]
    InvalidKey { path: PathBuf, key: String },
}

fn position_suffix(position: Option<(usize, usize)>) -> String {
    match position {
        Some((line, column)) => format!(" (line {line}, column {column})"),
        None => String::new(),
    }
}

/// Reads every entry of a `GRIM_CONFIG` value, in list order.
///
/// # Errors
///
/// The first failing entry's error; later entries are not read.
pub fn read_layers(value: &OsStr) -> Result<Vec<Layer>, LayerError> {
    let mut layers = Vec::new();
    for entry in std::env::split_paths(value) {
        if entry.as_os_str().is_empty() {
            continue;
        }
        layers.push(read_layer(entry)?);
    }
    Ok(layers)
}

fn read_layer(path: PathBuf) -> Result<Layer, LayerError> {
    if !path.is_absolute() {
        return Err(LayerError::Relative { entry: path });
    }
    let meta = std::fs::metadata(&path).map_err(|source| LayerError::Read {
        path: path.clone(),
        source,
    })?;
    // Opening a FIFO with no writer blocks forever, hanging every grim command.
    if !meta.is_file() {
        return Err(LayerError::NotRegular { path });
    }
    let text = crate::config::read_capped(&path).map_err(|err| match err.kind {
        ConfigErrorKind::FileTooLarge { limit, .. } => LayerError::TooLarge {
            path: path.clone(),
            limit,
        },
        ConfigErrorKind::Io(source) => LayerError::Read {
            path: path.clone(),
            source,
        },
        kind => LayerError::Read {
            path: path.clone(),
            source: std::io::Error::other(ConfigError::new(err.path, kind)),
        },
    })?;
    match toml::from_str::<toml::Table>(&text) {
        Ok(table) => Ok(Layer { path, table }),
        Err(err) => Err(LayerError::Parse {
            position: err.span().map(|span| line_column(&text, span.start)),
            path,
        }),
    }
}

fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let head = &text.as_bytes()[..offset.min(text.len())];
    let line = head.iter().filter(|&&b| b == b'\n').count() + 1;
    let line_start = head.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let column = String::from_utf8_lossy(&head[line_start..]).chars().count() + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::Path;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn join(paths: &[&Path]) -> OsString {
        std::env::join_paths(paths).unwrap()
    }

    #[test]
    fn empty_value_reads_nothing() {
        assert!(read_layers(OsStr::new("")).unwrap().is_empty());
    }

    #[test]
    fn empty_entries_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.toml", "x = 1\n");
        let sep = if cfg!(windows) { ";" } else { ":" };
        let mut value = OsString::from(sep);
        value.push(sep);
        value.push(&a);
        value.push(sep);
        let layers = read_layers(&value).unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].path, a);
    }

    #[test]
    fn semicolon_separates_entries_only_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.toml", "x = 1\n");
        let b = write(dir.path(), "b.toml", "x = 2\n");
        let mut value = a.clone().into_os_string();
        value.push(";");
        value.push(&b);
        let result = read_layers(&value);
        #[cfg(not(windows))]
        match result {
            Err(LayerError::Read { path, .. }) => assert_eq!(path.as_os_str(), value),
            other => panic!("expected one unsplit entry: {other:?}"),
        }
        #[cfg(windows)]
        {
            let paths: Vec<_> = result.unwrap().into_iter().map(|l| l.path).collect();
            assert_eq!(paths, [a, b]);
        }
    }

    #[test]
    fn order_is_kept_and_duplicates_read_twice() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.toml", "x = 1\n");
        let b = write(dir.path(), "b.toml", "x = 2\n");
        let layers = read_layers(&join(&[&a, &b, &a])).unwrap();
        let xs: Vec<i64> = layers.iter().map(|l| l.table["x"].as_integer().unwrap()).collect();
        assert_eq!(xs, [1, 2, 1]);
    }

    #[test]
    fn relative_entry_is_refused_even_when_it_exists() {
        // Cargo.toml exists in the test cwd; the refusal comes from the shape.
        let err = read_layers(OsStr::new("Cargo.toml")).unwrap_err();
        assert!(matches!(err, LayerError::Relative { .. }), "{err:?}");
        assert_eq!(
            err.to_string(),
            "GRIM_CONFIG entry 'Cargo.toml' is not an absolute path"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_rooted_and_drive_relative_are_refused() {
        for entry in [r"\foo", "C:foo"] {
            let err = read_layers(OsStr::new(entry)).unwrap_err();
            assert!(matches!(err, LayerError::Relative { .. }), "{entry}: {err:?}");
        }
    }

    #[test]
    fn missing_file_is_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_layers(dir.path().join("nope.toml").as_os_str()).unwrap_err();
        assert!(matches!(err, LayerError::Read { .. }), "{err:?}");
        assert!(err.to_string().starts_with("cannot read GRIM_CONFIG file '"));
    }

    #[test]
    fn directory_is_not_regular() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_layers(dir.path().as_os_str()).unwrap_err();
        assert!(matches!(err, LayerError::NotRegular { .. }), "{err:?}");
        assert!(err.to_string().ends_with("' is not a regular file"));
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_not_regular_and_does_not_block() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        let status = std::process::Command::new("mkfifo").arg(&fifo).status().unwrap();
        assert!(status.success());
        let err = read_layers(fifo.as_os_str()).unwrap_err();
        assert!(matches!(err, LayerError::NotRegular { .. }), "{err:?}");
    }

    #[test]
    fn oversize_file_is_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.toml");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(crate::config::FILE_SIZE_LIMIT_BYTES + 1)
            .unwrap();
        let err = read_layers(path.as_os_str()).unwrap_err();
        assert!(matches!(err, LayerError::TooLarge { .. }), "{err:?}");
        assert!(err.to_string().ends_with(&format!(
            "' is larger than {} bytes",
            crate::config::FILE_SIZE_LIMIT_BYTES
        )));
    }

    #[test]
    fn parse_error_gives_position_and_never_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "bad.toml",
            "[telemetry.headers]\nauthorization = \"Bearer SENTINEL-SECRET-7f3a\n",
        );
        let err = read_layers(path.as_os_str()).unwrap_err();
        let shown = err.to_string();
        assert!(shown.contains("line 2"), "{shown}");
        assert!(shown.contains("column"), "{shown}");
        assert!(!shown.contains("SENTINEL"), "{shown}");
        assert!(!format!("{err:?}").contains("SENTINEL"));
    }

    #[test]
    fn line_column_is_one_based() {
        assert_eq!(line_column("ab\ncd", 0), (1, 1));
        assert_eq!(line_column("ab\ncd", 4), (2, 2));
    }

    #[test]
    fn valid_file_keeps_every_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "ok.toml",
            "[telemetry]\nendpoint = \"x\"\n[other]\ny = true\n",
        );
        let layers = read_layers(path.as_os_str()).unwrap();
        assert_eq!(layers[0].table["telemetry"]["endpoint"].as_str(), Some("x"));
        assert_eq!(layers[0].table["other"]["y"].as_bool(), Some(true));
    }

    #[test]
    fn layer_debug_prints_key_names_only() {
        let table: toml::Table =
            toml::from_str("[telemetry.headers]\nauthorization = \"Bearer SENTINEL-dbg\"\n").unwrap();
        let layer = Layer {
            path: PathBuf::from("/x.toml"),
            table,
        };
        let shown = format!("{layer:?}");
        assert!(!shown.contains("SENTINEL"), "{shown}");
        assert_eq!(shown, r#"Layer { path: "/x.toml", keys: ["telemetry"] }"#);
    }

    #[test]
    fn invalid_key_display_names_key_only() {
        let err = LayerError::InvalidKey {
            path: PathBuf::from("/f.toml"),
            key: "telemetry.timeout".into(),
        };
        assert_eq!(
            err.to_string(),
            "GRIM_CONFIG file '/f.toml' has an unusable value for 'telemetry.timeout'"
        );
    }
}
