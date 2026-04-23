use reqwest::{
    Body, Client, Response,
    header::{CONTENT_TYPE, HeaderValue, COOKIE},
};
use serde::{Serialize, Deserialize, de::DeserializeOwned};
use serde_json::to_value;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::future::Future;
use url::Url;

use crate::sdk::utils::Error;

/// Returns true if the user has set OM_FORCE_CURL=1 (or any non-empty value other than "0").
/// When enabled, the SDK skips the reqwest attempt entirely and goes straight to the curl
/// fallback. This saves a round-trip on networks where reqwest is known to be blocked by
/// strict TLS/proxy fingerprinting at the Xnode Manager nginx layer.
fn force_curl() -> bool {
    match std::env::var("OM_FORCE_CURL") {
        Ok(v) => !v.is_empty() && v != "0",
        Err(_) => false,
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub reqwest_client: Client,
    pub base_url: String,
    pub domain: String,
    pub cookies: Vec<String>,
    /// Set when domain differs from the URL host (IP-only xnodes).
    pub host_override: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistedSession {
    pub url: String,
    pub cookies: Vec<String>,
    /// Optional host override for IP-only xnodes. When set, this is used
    /// as the Host header and auth domain instead of extracting from url.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_override: Option<String>,
}

impl Session {
    // ─── Profile directory ─────────────────────────────────────────

    /// Returns `~/.openmesh/profiles/`
    fn profiles_dir() -> Result<PathBuf, Error> {
        let mut dir = dirs_next::home_dir()
            .ok_or_else(|| Error::OutputError("Could not find home directory".to_string()))?;
        dir.push(".openmesh");
        dir.push("profiles");
        Ok(dir)
    }

    /// Returns the path for a named profile: `~/.openmesh/profiles/<name>.json`
    fn profile_path(name: &str) -> Result<PathBuf, Error> {
        let mut path = Self::profiles_dir()?;
        path.push(format!("{}.json", name));
        Ok(path)
    }

    /// Returns `~/.openmesh/default` — a text file containing the default profile name.
    fn default_profile_path() -> Result<PathBuf, Error> {
        let mut path = dirs_next::home_dir()
            .ok_or_else(|| Error::OutputError("Could not find home directory".to_string()))?;
        path.push(".openmesh");
        path.push("default");
        Ok(path)
    }

    /// Legacy single-session file path.
    pub fn get_session_path() -> Result<PathBuf, Error> {
        let mut path = dirs_next::home_dir()
            .ok_or_else(|| Error::OutputError("Could not find home directory".to_string()))?;
        path.push(".openmesh_session.cookie");
        Ok(path)
    }

    // ─── Profile management ────────────────────────────────────────

    /// List all profile names.
    pub fn list_profiles() -> Result<Vec<String>, Error> {
        let dir = Self::profiles_dir()?;
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| Error::OutputError(e.to_string()))? {
            let entry = entry.map_err(|e| Error::OutputError(e.to_string()))?;
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    names.push(stem.to_string());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    /// Get the default profile name, if set.
    pub fn get_default_profile() -> Result<Option<String>, Error> {
        let path = Self::default_profile_path()?;
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)
            .map_err(|e| Error::OutputError(e.to_string()))?;
        let name = content.trim().to_string();
        if name.is_empty() { Ok(None) } else { Ok(Some(name)) }
    }

    /// Set the default profile name.
    pub fn set_default_profile(name: &str) -> Result<(), Error> {
        let path = Self::default_profile_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::OutputError(e.to_string()))?;
        }
        fs::write(&path, name).map_err(|e| Error::OutputError(e.to_string()))?;
        Ok(())
    }

    /// Delete a named profile.
    pub fn delete_profile(name: &str) -> Result<(), Error> {
        let path = Self::profile_path(name)?;
        if path.exists() {
            fs::remove_file(&path).map_err(|e| Error::OutputError(e.to_string()))?;
        }
        // If this was the default, clear the default
        if let Ok(Some(default)) = Self::get_default_profile() {
            if default == name {
                let _ = fs::remove_file(Self::default_profile_path()?);
            }
        }
        Ok(())
    }

    // ─── Load / Save ───────────────────────────────────────────────

    fn from_persisted(persisted: PersistedSession) -> Result<Self, Error> {
        let url_parsed = Url::parse(&persisted.url)
            .map_err(|e| Error::OutputError(e.to_string()))?;
        let url_host = url_parsed.host_str()
            .ok_or_else(|| Error::OutputError("Invalid URL in session".to_string()))?
            .to_string();
        // Use host_override if set (for IP-only xnodes needing manager.xnode.local)
        let domain = persisted.host_override.unwrap_or(url_host);
        let origin = format!("{}://{}", url_parsed.scheme(), domain);

        let mut headers = reqwest::header::HeaderMap::new();
        if !persisted.cookies.is_empty() {
            let cookie_header = persisted.cookies.join("; ");
            headers.insert(COOKIE, cookie_header.parse()
                .map_err(|_| Error::OutputError("Invalid cookie header".to_string()))?);
        }
        headers.insert("Host", domain.parse()
            .map_err(|_| Error::OutputError("Invalid domain".to_string()))?);
        headers.insert("Origin", origin.parse()
            .map_err(|_| Error::OutputError("Invalid origin".to_string()))?);

        // Determine if host_override is in play (IP-only xnode bootstrap).
        // Only bypass TLS cert verification for IP-only sessions — domain-based
        // sessions must validate the cert to prevent MITM of session cookies.
        let url_host_check = url_parsed.host_str().unwrap_or("").to_string();
        let host_override = if domain != url_host_check {
            Some(domain.clone())
        } else {
            None
        };
        let is_ip_only = host_override.is_some();

        let mut client_builder = Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::limited(10));
        if is_ip_only {
            client_builder = client_builder.danger_accept_invalid_certs(true);
        }
        let client = client_builder.build().map_err(Error::ReqwestError)?;

        Ok(Self {
            reqwest_client: client,
            base_url: persisted.url,
            domain,
            cookies: persisted.cookies,
            host_override,
        })
    }

    /// Load a named profile.
    pub fn load_profile(name: &str) -> Result<Self, Error> {
        let path = Self::profile_path(name)?;
        if !path.exists() {
            return Err(Error::OutputError(format!(
                "Profile '{}' not found. Run: om profile login {} -u <URL>",
                name, name
            )));
        }
        let content = fs::read_to_string(&path)
            .map_err(|e| Error::OutputError(e.to_string()))?;
        let persisted: PersistedSession = serde_json::from_str(&content)
            .map_err(Error::SerdeJsonError)?;
        Self::from_persisted(persisted)
    }

    /// Load the default session. Priority:
    /// 1. Named profile (if --profile was passed or default is set)
    /// 2. Legacy `~/.openmesh_session.cookie`
    pub fn load() -> Result<Self, Error> {
        // Try default profile first
        if let Ok(Some(default)) = Self::get_default_profile() {
            let path = Self::profile_path(&default)?;
            if path.exists() {
                return Self::load_profile(&default);
            }
        }

        // Fall back to legacy session file
        let path = Self::get_session_path()?;
        if !path.exists() {
            return Err(Error::OutputError("No session found. Run 'om login' or 'om profile login <name> -u <URL>'".to_string()));
        }
        let content = fs::read_to_string(path)
            .map_err(|e| Error::OutputError(e.to_string()))?;
        let persisted: PersistedSession = serde_json::from_str(&content)
            .map_err(Error::SerdeJsonError)?;
        Self::from_persisted(persisted)
    }

    fn to_persisted(&self) -> PersistedSession {
        PersistedSession {
            url: self.base_url.clone(),
            cookies: self.cookies.clone(),
            host_override: self.host_override.clone(),
        }
    }

    /// Save to a named profile.
    pub fn save_profile(&self, name: &str) -> Result<(), Error> {
        let path = Self::profile_path(name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::OutputError(e.to_string()))?;
        }
        let content = serde_json::to_string(&self.to_persisted()).map_err(Error::SerdeJsonError)?;
        fs::write(&path, content).map_err(|e| Error::OutputError(e.to_string()))?;
        Ok(())
    }

    /// Save to the legacy session file (backward compat).
    pub fn save(&self) -> Result<(), Error> {
        let path = Self::get_session_path()?;
        let content = serde_json::to_string(&self.to_persisted()).map_err(Error::SerdeJsonError)?;
        fs::write(path, content).map_err(|e| Error::OutputError(e.to_string()))?;
        Ok(())
    }
}

pub trait ResponseData<T: Sized> {
    fn from_response(response: Response) -> impl Future<Output = Result<T, Error>>;
}
impl<Data: DeserializeOwned> ResponseData<Data> for Data {
    async fn from_response(response: Response) -> Result<Self, Error> {
        let bytes = response.bytes().await.map_err(Error::ReqwestError)?;
        match serde_json::from_slice(&bytes) {
            Ok(data) => Ok(data),
            Err(e) => {
                let body = String::from_utf8_lossy(&bytes);
                Err(Error::OutputError(format!("Failed to parse JSON: {}. Body: {}", e, body)))
            }
        }
    }
}

pub trait QueryData {
    fn create_query(&self) -> Result<Vec<(String, String)>, Error>;
}
impl<Data: Serialize> QueryData for Data {
    fn create_query(&self) -> Result<Vec<(String, String)>, Error> {
        Ok(to_value(self)
            .map_err(Error::SerdeJsonError)?
            .as_object()
            .map(|x| {
                x.into_iter()
                    .filter_map(|(key, value)| match value {
                        serde_json::Value::String(s) => Some((key.clone(), s.clone())),
                        serde_json::Value::Number(n) => Some((key.clone(), n.to_string())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or(vec![]))
    }
}

pub trait BodyData {
    fn create_body(&self) -> Result<Body, Error>;
}
impl<Data: Serialize> BodyData for Data {
    fn create_body(&self) -> Result<Body, Error> {
        Ok(serde_json::to_vec(&self)
            .map_err(Error::SerdeJsonError)?
            .into())
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Empty {}

#[derive(Debug, Clone)]
pub struct SessionGetInput<'a, Path, Query: QueryData> {
    pub session: &'a Session,
    pub path: Path,
    pub query: Query,
}
pub type SessionGetOutput<Output> = Result<Output, Error>;
#[derive(Debug, Clone)]
pub struct SessionPostInput<'a, Path, Data: BodyData> {
    pub session: &'a Session,
    pub path: Path,
    pub data: Data,
}
pub type SessionPostOutput<Output> = Result<Output, Error>;

impl<'a> SessionGetInput<'a, Empty, Empty> {
    pub fn new(session: &'a Session) -> Self {
        Self {
            session,
            path: Empty::default(),
            query: Empty::default(),
        }
    }
}
impl<'a, Path> SessionGetInput<'a, Path, Empty> {
    pub fn new_with_path(session: &'a Session, path: Path) -> Self {
        Self {
            session,
            path,
            query: Empty::default(),
        }
    }
}
impl<'a, Query: QueryData> SessionGetInput<'a, Empty, Query> {
    pub fn new_with_query(session: &'a Session, query: Query) -> Self {
        Self {
            session,
            path: Empty::default(),
            query,
        }
    }
}

impl<'a> SessionPostInput<'a, Empty, Empty> {
    pub fn new(session: &'a Session) -> Self {
        Self {
            session,
            path: Empty::default(),
            data: Empty::default(),
        }
    }
}
impl<'a, Path> SessionPostInput<'a, Path, Empty> {
    pub fn new_with_path(session: &'a Session, path: Path) -> Self {
        Self {
            session,
            path,
            data: Empty::default(),
        }
    }
}
impl<'a, Data: BodyData> SessionPostInput<'a, Empty, Data> {
    pub fn new_with_data(session: &'a Session, data: Data) -> Self {
        Self {
            session,
            path: Empty::default(),
            data,
        }
    }
}

pub async fn session_get<
    Output: ResponseData<Output> + DeserializeOwned,
    Path,
    Query: QueryData,
    PathOutput: Into<String>,
>(
    input: SessionGetInput<'_, Path, Query>,
    scope: String,
    path: fn(Path) -> PathOutput,
) -> SessionGetOutput<Output> {
    let session = input.session;
    let path_str: String = path(input.path).into();
    let url = format!(
        "{}{}{}",
        session.base_url,
        scope,
        path_str
    );
    let query_params = input.query.create_query()?;
    
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("path", (format!("{}{}", scope, path_str)).parse().unwrap());
    headers.insert("Origin", "https://xnode.openmesh.network".parse().unwrap());
    headers.insert("Referer", "https://xnode.openmesh.network/".parse().unwrap());

    // Honor OM_FORCE_CURL: skip the reqwest attempt entirely when set.
    // The known-failure path (nginx 400 on Rust client) makes the reqwest call a wasted RTT.
    let resp = if force_curl() {
        None
    } else {
        Some(session
            .reqwest_client
            .get(&url)
            .headers(headers)
            .query(&query_params)
            .send()
            .await)
    };

    match resp {
        Some(Ok(r)) if r.status().as_u16() != 400 => {
            Output::from_response(r).await
        }
        _ => {
            // FALLBACK TO CURL
            let mut curl = std::process::Command::new("curl");
            curl.arg("-s").arg("-L");
            // Only bypass TLS for IP-only xnode bootstrap sessions. Domain-
            // verified sessions (community.openxai.org etc.) must validate
            // the cert — otherwise an on-path attacker could MITM the API.
            if session.host_override.is_some() {
                curl.arg("-k");
            }
            curl.arg("-H").arg(format!("Host: {}", session.domain));
            curl.arg("-H").arg("Origin: https://xnode.openmesh.network");
            curl.arg("-H").arg("Referer: https://xnode.openmesh.network/");
            curl.arg("-H").arg(format!("path: {}{}", scope, path_str));
            curl.arg("-A").arg("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36");
            
            // Always pass cookies inline from the session — this ensures
            // profile-specific cookies are used, not the legacy jar file.
            if !session.cookies.is_empty() {
                curl.arg("-b").arg(session.cookies.join("; "));
            } else {
                let mut cookie_jar_path = Session::get_session_path()?;
                cookie_jar_path.set_extension("jar");
                if cookie_jar_path.exists() {
                    curl.arg("-b").arg(&cookie_jar_path);
                }
            }

            let mut final_url = url.clone();
            if !query_params.is_empty() {
                final_url.push('?');
                for (i, (k, v)) in query_params.iter().enumerate() {
                    if i > 0 { final_url.push('&'); }
                    final_url.push_str(k);
                    final_url.push('=');
                    final_url.push_str(v);
                }
            }
            
            let output = curl.arg(&final_url).output().map_err(|e| Error::OutputError(e.to_string()))?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            
            if output.status.success() {
                match serde_json::from_str::<Output>(&stdout) {
                    Ok(data) => Ok(data),
                    Err(e) => {
                        if stdout.to_lowercase().contains("unauthorized") || stdout.to_lowercase().contains("login") {
                            Err(Error::OutputError("Session expired or unauthorized. Please run 'om login' again.".to_string()))
                        } else {
                            Err(Error::OutputError(format!("Failed to parse JSON: {}. Body: {}", e, stdout)))
                        }
                    }
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(Error::OutputError(format!("Request failed with status {}. Error: {}", output.status, stderr)))
            }
        }
    }
}

pub async fn session_post<
    Output: ResponseData<Output> + DeserializeOwned,
    Path,
    Data: BodyData + Serialize,
    PathOutput: Into<String>,
>(
    input: SessionPostInput<'_, Path, Data>,
    scope: String,
    path: fn(Path) -> PathOutput,
) -> SessionPostOutput<Output> {
    let session = input.session;
    let path_str: String = path(input.path).into();
    let url = format!("{}{}{}", session.base_url, scope, path_str);

    // Serialize body once. We need the bytes both for reqwest and the curl fallback.
    let body_bytes = serde_json::to_vec(&input.data).map_err(Error::SerdeJsonError)?;

    // Honor OM_FORCE_CURL: skip the reqwest attempt entirely when set.
    let resp = if force_curl() {
        None
    } else {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("path", (format!("{}{}", scope, path_str)).parse().unwrap());
        headers.insert("Origin", "https://xnode.openmesh.network".parse().unwrap());
        headers.insert("Referer", "https://xnode.openmesh.network/".parse().unwrap());
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        Some(session
            .reqwest_client
            .post(&url)
            .headers(headers)
            .body(Body::from(body_bytes.clone()))
            .send()
            .await)
    };

    match resp {
        Some(Ok(r)) if r.status().as_u16() != 400 => {
            Output::from_response(r).await
        }
        _ => {
            // FALLBACK TO CURL
            //
            // The Xnode Manager nginx proxy rejects requests from the Rust reqwest client
            // (likely TLS fingerprint or HTTP/2 normalization). System curl gets through
            // with a browser User-Agent, so we shell out and pipe the JSON body via stdin
            // to avoid argv length limits and shell-escaping pitfalls.
            let mut curl = Command::new("curl");
            curl.arg("-s").arg("-L");
            // See note in session_get: -k is IP-only bootstrap only.
            if session.host_override.is_some() {
                curl.arg("-k");
            }
            curl.arg("-X").arg("POST");
            curl.arg("-H").arg(format!("Host: {}", session.domain));
            curl.arg("-H").arg("Origin: https://xnode.openmesh.network");
            curl.arg("-H").arg("Referer: https://xnode.openmesh.network/");
            curl.arg("-H").arg(format!("path: {}{}", scope, path_str));
            curl.arg("-H").arg("Content-Type: application/json");
            curl.arg("-A").arg("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36");

            // Pass cookies inline from the session for profile support.
            if !session.cookies.is_empty() {
                curl.arg("-b").arg(session.cookies.join("; "));
            }

            // Pipe the body via stdin: `curl --data-binary @-`
            curl.arg("--data-binary").arg("@-");
            curl.arg(&url);
            curl.stdin(Stdio::piped());
            curl.stdout(Stdio::piped());
            curl.stderr(Stdio::piped());

            let mut child = curl.spawn().map_err(|e| Error::OutputError(format!("curl spawn failed: {}", e)))?;
            {
                let stdin = child
                    .stdin
                    .as_mut()
                    .ok_or_else(|| Error::OutputError("Failed to open curl stdin".to_string()))?;
                stdin
                    .write_all(&body_bytes)
                    .map_err(|e| Error::OutputError(format!("Failed to write body to curl: {}", e)))?;
            }
            let output = child
                .wait_with_output()
                .map_err(|e| Error::OutputError(format!("curl wait failed: {}", e)))?;

            let stdout = String::from_utf8_lossy(&output.stdout);

            if output.status.success() {
                // Some POST endpoints return an empty body on success (e.g. file writes).
                // Try to parse as the requested Output type, but if the body is empty and
                // Output is `Empty`/unit-like, fall back to parsing "{}".
                let parse_target = if stdout.trim().is_empty() { "{}" } else { stdout.as_ref() };
                match serde_json::from_str::<Output>(parse_target) {
                    Ok(data) => Ok(data),
                    Err(e) => {
                        if stdout.to_lowercase().contains("unauthorized") || stdout.to_lowercase().contains("login") {
                            Err(Error::OutputError("Session expired or unauthorized. Please run 'om login' again.".to_string()))
                        } else {
                            Err(Error::OutputError(format!("Failed to parse JSON: {}. Body: {}", e, stdout)))
                        }
                    }
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(Error::OutputError(format!("Request failed with status {}. Stderr: {}. Body: {}", output.status, stderr, stdout)))
            }
        }
    }
}
