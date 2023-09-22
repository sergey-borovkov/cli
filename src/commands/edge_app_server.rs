use crate::commands::ignorer::Ignorer;
use anyhow::Result;
use futures::future::{self, BoxFuture, FutureExt};
use std::collections::{BTreeMap, HashMap};
use std::fs;

use serde::{Deserialize, Serialize};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use warp::reject::Reject;
use warp::{Filter, Rejection, Reply};

pub const MOCK_DATA_FILENAME: &str = "mock-data.yml";

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub enum Value {
    Str(String),
    Array(Vec<String>),
}

pub async fn run_server(
    path: &Path,
    secrets: Vec<(String, String)>,
) -> Result<String, anyhow::Error> {
    let secrets_val = secrets
        .iter()
        .map(|(k, v)| (k.clone(), Value::Str(v.clone())))
        .collect::<Vec<(_, _)>>();

    let dir_path = Arc::new(path.to_path_buf());

    let ignorer = Arc::new(Ignorer::new(&*dir_path)?);

    let directory = warp::path("edge")
        .and(warp::path("1"))
        .and(warp::fs::dir(dir_path.as_path().to_owned()))
        .and_then(
            move |file: warp::filters::fs::File| -> BoxFuture<'static, Result<_, Rejection>> {
                if ignorer.is_ignored(file.path()) {
                    future::err(warp::reject::not_found()).boxed()
                } else {
                    future::ok(file).boxed()
                }
            },
        );

    let secrets_map: Vec<(_, _)> = secrets_val.into_iter().collect();
    let secrets_clone = secrets_map;

    let virtual_file = warp::path("edge")
        .and(warp::path("1"))
        .and(warp::path("screenly.js"))
        .and(warp::query::<HashMap<String, String>>())
        .and_then({
            let dir_path = dir_path;
            move |params: HashMap<String, String>| {
                let dir_path = dir_path.clone();
                let secrets_clone = secrets_clone.clone();
                async move {
                    if let Some(version) = params.get("version") {
                        if version == "1" {
                            return generate_content(dir_path, &secrets_clone).await;
                        }
                    }
                    Err(warp::reject::not_found())
                }
            }
        });

    let routes = directory.or(virtual_file);

    let server = warp::serve(routes);
    let addr: std::net::SocketAddr = ([127, 0, 0, 1], 0).into();

    let (addr, server_future) = server.bind_ephemeral(addr);

    tokio::task::spawn(server_future);

    Ok(format!("http://{}/edge/1", addr))
}

#[derive(Debug)]
struct WarpError(anyhow::Error);

impl Reject for WarpError {}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Metadata {
    coordinates: Vec<String>,
    hardware: String,
    hostname: String,
    location: String,
    screen_name: String,
    tags: Vec<String>,
}
impl Default for Metadata {
    fn default() -> Self {
        Metadata {
            coordinates: vec!["37.3861".to_string(), "-122.0839".to_string()],
            hardware: "x86".to_string(),
            hostname: "srly-t6kb0ta1jrd9o0w".to_string(),
            location: "Silicon Valley, USA".to_string(),
            screen_name: "Code Cafe Display".to_string(),
            tags: vec!["All Screens".to_string()],
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct MockData {
    metadata: Metadata,
    settings: Vec<(String, Value)>,
}

#[derive(Debug, Default, Deserialize, Clone)]
struct MockDataStr {
    metadata: Metadata,
    settings: BTreeMap<String, String>,
}

impl MockData {
    fn new_from_str(mock_data_str: &MockDataStr) -> Self {
        let settings_val = mock_data_str
            .settings
            .iter()
            .map(|(k, v)| {
                if v.len() > 2
                    && v.chars().nth(0) == Some('[')
                    && v.chars().nth(v.len() - 1) == Some(']')
                {
                    let v = &v[1..(v.len() - 1)];
                    let v = v
                        .split(',')
                        .map(|s| s.trim().replace(['"', '\''], ""))
                        .collect::<Vec<_>>();
                    (k.clone(), Value::Array(v.clone()))
                } else {
                    (k.clone(), Value::Str(v.clone()))
                }
            })
            .collect::<Vec<(_, _)>>();

        MockData {
            metadata: mock_data_str.metadata.clone(),
            settings: settings_val,
        }
    }
}

async fn generate_content(
    dir_path: Arc<PathBuf>,
    secrets: &[(String, Value)],
) -> Result<impl Reply, Rejection> {
    let file_path = dir_path.join(MOCK_DATA_FILENAME);

    let content = if file_path.exists() {
        fs::read_to_string(&file_path).unwrap_or("".to_string())
    } else {
        eprintln!(
            "Mock data does not exist. Use \"edge-app run --generate-mock-data\" to create mock data."
        );
        return Err(warp::reject::not_found());
    };

    let data_str: MockDataStr = match serde_yaml::from_str(&content) {
        Ok(data_str) => data_str,
        Err(e) => {
            eprintln!("Mock data deserialization Error: {:?}. Use \"edge-app run --generate-mock-data\" to create mock data.", e);
            return Err(warp::reject::not_found());
        }
    };

    let data = MockData::new_from_str(&data_str);
    let js_output = format_js(data, secrets);

    Ok(warp::reply::html(js_output))
}

fn format_js(data: MockData, secrets: &[(String, Value)]) -> String {
    format!(
        "var screenly = {{\n{metadata},\n{settings},\n{secrets},\n{cors_proxy}\n}};",
        metadata = format_section("metadata", &hashmap_from_metadata(&data.metadata)),
        settings = format_section("settings", &data.settings),
        secrets = format_section("secrets", secrets),
        cors_proxy = "    cors_proxy_url: \"http://127.0.0.1:8080\""
    )
}

fn hashmap_from_metadata(metadata: &Metadata) -> Vec<(String, Value)> {
    let result = vec![
        (
            "coordinates".to_string(),
            Value::Array(metadata.coordinates.clone()),
        ),
        (
            "hardware".to_string(),
            Value::Str(metadata.hardware.clone()),
        ),
        (
            "hostname".to_string(),
            Value::Str(metadata.hostname.clone()),
        ),
        (
            "location".to_string(),
            Value::Str(metadata.location.clone()),
        ),
        (
            "screen_name".to_string(),
            Value::Str(metadata.screen_name.clone()),
        ),
        (
            "tags".to_string(),
            Value::Array(
                metadata
                    .tags
                    .iter()
                    .map(|tag| tag.to_string())
                    .collect::<Vec<String>>(),
            ),
        ),
    ];
    result
}

fn format_section(name: &str, items: &[(String, Value)]) -> String {
    let content = items
        .iter()
        .map(|(k, v)| match v {
            Value::Str(s) => format!("        \"{}\": \"{}\"", k, s),
            Value::Array(arr) => format!(
                "        \"{}\": [{}]",
                k,
                arr.iter()
                    .map(|item| format!("\"{}\"", item))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!("    {}: {{\n{}\n    }}", name, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn setup_temp_dir_with_mock_data() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join(MOCK_DATA_FILENAME);
        let mut file = fs::File::create(file_path).unwrap();

        writeln!(
            file,
            r#"
metadata:
  coordinates:
  - '37.3861'
  - '-122.0839'
  hardware: x86
  hostname: srly-t6kb0ta1jrd9o0w
  location: Silicon Valley, USA
  screen_name: Code Cafe Display
  tags:
  - All Screens
settings:
  enable_analytics: 'true'
  override_timezone: ''
  tag_manager_id: ''
"#
        )
        .unwrap();

        dir
    }

    #[tokio::test]
    async fn test_server_should_serve_correct_mock_data() {
        let dir = setup_temp_dir_with_mock_data();
        let dir_path = dir.path().to_path_buf();

        let address = run_server(&dir_path, vec![("key".to_string(), "value".to_string())])
            .await
            .unwrap();
        let resp = reqwest::get(format!("{}/screenly.js?version=1", address))
            .await
            .unwrap();
        let content = resp.text().await.unwrap();
        let expected_content = r#"var screenly = {
    metadata: {
        "coordinates": ["37.3861", "-122.0839"],
        "hardware": "x86",
        "hostname": "srly-t6kb0ta1jrd9o0w",
        "location": "Silicon Valley, USA",
        "screen_name": "Code Cafe Display",
        "tags": ["All Screens"]
    },
    settings: {
        "enable_analytics": "true",
        "override_timezone": "",
        "tag_manager_id": ""
    },
    secrets: {
        "key": "value"
    },
    cors_proxy_url: "http://127.0.0.1:8080"
};"#;
        assert_eq!(content, expected_content);
    }

    #[tokio::test]
    async fn test_server_without_mock_data() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();

        let address = run_server(&dir_path, vec![("key".to_string(), "value".to_string())])
            .await
            .unwrap();
        let resp = reqwest::get(format!("{}/screenly.js?version=1", address))
            .await
            .unwrap();

        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn test_server_when_invalid_version_requested_should_return() {
        let dir = setup_temp_dir_with_mock_data();
        let dir_path = dir.path().to_path_buf();

        let address = run_server(&dir_path, vec![("key".to_string(), "value".to_string())])
            .await
            .unwrap();

        let resp = reqwest::get(format!("{}/screenly.js?version=2", address))
            .await
            .unwrap();

        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn test_server_mockdata_str_should_obtain_mockdata_from_str() {
        let metadata = Metadata::default();
        let mut settings = BTreeMap::new();

        settings.insert("enable_analytics".to_string(), "true".to_string());
        settings.insert("override_timezone".to_string(), "".to_string());
        settings.insert("tag_manager_id".to_string(), "".to_string());

        let mockdata_str = MockDataStr { metadata, settings };
        let mockdata = MockData::new_from_str(&mockdata_str);
        let (k, v) = mockdata.settings.first().unwrap();

        assert_eq!(*k, "enable_analytics");
        assert_eq!(*v, Value::Str("true".to_string()));
    }
}
