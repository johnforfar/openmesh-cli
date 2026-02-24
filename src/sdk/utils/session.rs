use reqwest::{
    Body, Client, Response,
    header::{CONTENT_TYPE, HeaderValue, COOKIE},
};
use serde::{Serialize, Deserialize, de::DeserializeOwned};
use serde_json::to_value;
use std::fs;
use std::path::PathBuf;
use std::future::Future;
use url::Url;

use crate::sdk::utils::Error;

#[derive(Debug, Clone)]
pub struct Session {
    pub reqwest_client: Client,
    pub base_url: String,
    pub domain: String,
    pub cookies: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistedSession {
    pub url: String,
    pub cookies: Vec<String>,
}

impl Session {
    pub fn get_session_path() -> Result<PathBuf, Error> {
        let mut path = dirs_next::home_dir().ok_or_else(|| Error::OutputError("Could not find home directory".to_string()))?;
        path.push(".openmesh_session.cookie");
        Ok(path)
    }

    pub fn load() -> Result<Self, Error> {
        let path = Self::get_session_path()?;
        if !path.exists() {
            return Err(Error::OutputError("No session found".to_string()));
        }
        let content = fs::read_to_string(path).map_err(|e| Error::OutputError(e.to_string()))?;
        let persisted: PersistedSession = serde_json::from_str(&content).map_err(Error::SerdeJsonError)?;
        
        let url_parsed = Url::parse(&persisted.url).map_err(|e| Error::OutputError(e.to_string()))?;
        let domain = url_parsed.host_str().ok_or_else(|| Error::OutputError("Invalid URL in session".to_string()))?.to_string();
        let origin = format!("{}://{}", url_parsed.scheme(), domain);

        let mut headers = reqwest::header::HeaderMap::new();
        if !persisted.cookies.is_empty() {
            let cookie_header = persisted.cookies.join("; ");
            headers.insert(COOKIE, cookie_header.parse().map_err(|_| Error::OutputError("Invalid cookie header".to_string()))?);
        }
        
        headers.insert("Host", domain.parse().map_err(|_| Error::OutputError("Invalid domain".to_string()))?);
        headers.insert("Origin", origin.parse().map_err(|_| Error::OutputError("Invalid origin".to_string()))?);

        let client = Client::builder()
            .danger_accept_invalid_certs(true)
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(Error::ReqwestError)?;

        Ok(Self {
            reqwest_client: client,
            base_url: persisted.url,
            domain,
            cookies: persisted.cookies,
        })
    }

    pub fn save(&self) -> Result<(), Error> {
        let path = Self::get_session_path()?;
        let persisted = PersistedSession {
            url: self.base_url.clone(),
            cookies: self.cookies.clone(),
        };
        let content = serde_json::to_string(&persisted).map_err(Error::SerdeJsonError)?;
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
    headers.insert("Path", (format!("{}{}", scope, path_str)).parse().unwrap());
    headers.insert("Origin", (format!("https://{}", session.domain)).parse().unwrap());
    headers.insert("Referer", (format!("https://{}{}{}", session.domain, scope, path_str)).parse().unwrap());

    let resp = session
        .reqwest_client
        .get(&url)
        .headers(headers)
        .query(&query_params)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().as_u16() != 400 => {
            Output::from_response(r).await
        }
        _ => {
            // FALLBACK TO CURL
            let mut curl = std::process::Command::new("curl");
            curl.arg("-s").arg("-L");
            curl.arg("-H").arg(format!("Host: {}", session.domain));
            curl.arg("-H").arg(format!("Origin: https://{}", session.domain));
            curl.arg("-H").arg(format!("Referer: https://{}{}{}", session.domain, scope, path_str));
            curl.arg("-H").arg(format!("path: {}{}", scope, path_str));
            curl.arg("-A").arg("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36");
            
            let mut cookie_jar_path = Session::get_session_path()?;
            cookie_jar_path.set_extension("jar");
            if cookie_jar_path.exists() {
                curl.arg("-b").arg(&cookie_jar_path);
            } else if !session.cookies.is_empty() {
                curl.arg("-b").arg(session.cookies.join("; "));
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
    Data: BodyData,
    PathOutput: Into<String>,
>(
    input: SessionPostInput<'_, Path, Data>,
    scope: String,
    path: fn(Path) -> PathOutput,
) -> SessionPostOutput<Output> {
    let session = input.session;
    let url = format!(
        "{}{}{}",
        session.base_url,
        scope,
        path(input.path).into()
    );
    
    let resp = session
        .reqwest_client
        .post(&url)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .body(input.data.create_body()?)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().as_u16() != 400 => {
            Output::from_response(r).await
        }
        _ => {
            // FALLBACK TO CURL
            let mut curl = std::process::Command::new("curl");
            curl.arg("-s").arg("-L").arg("-X").arg("POST");
            curl.arg("-H").arg(format!("Host: {}", session.domain));
            curl.arg("-H").arg(format!("Origin: https://{}", session.domain));
            curl.arg("-H").arg("Content-Type: application/json");
            curl.arg("-A").arg("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36");
            
            let mut cookie_jar_path = Session::get_session_path()?;
            cookie_jar_path.set_extension("jar");
            if cookie_jar_path.exists() {
                curl.arg("-b").arg(cookie_jar_path);
            } else if !session.cookies.is_empty() {
                curl.arg("-b").arg(session.cookies.join("; "));
            }
            
            Err(Error::OutputError("Post fallback not fully implemented for generic data".to_string()))
        }
    }
}
