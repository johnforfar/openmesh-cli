use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::sdk::utils::{Empty, Error, Session, SessionPostInput, SessionPostOutput, session_post};

pub fn scope() -> String {
    "/xnode-auth".to_string()
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct User {
    user: String,
    signature: Option<String>,
    timestamp: Option<String>,
}
impl User {
    pub fn new(user: String) -> Self {
        Self {
            user,
            signature: None,
            timestamp: None,
        }
    }

    pub fn with_signature(user: String, signature: String, timestamp: String) -> Self {
        Self {
            user,
            signature: Some(signature),
            timestamp: Some(timestamp),
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct LoginInput {
    pub base_url: String,
    pub user: User,
}
pub type LoginOutput = Result<Session, Error>;
pub async fn login(input: LoginInput) -> LoginOutput {
    let client = Client::builder()
        .cookie_store(true)
        .build()
        .map_err(Error::ReqwestError)?;

    client
        .post(format!("{}{}/api/login", input.base_url, scope()))
        .json(&input.user)
        .send()
        .await
        .map_err(Error::ReqwestError)?;

    use url::Url;
    let url_parsed = Url::parse(&input.base_url).map_err(|_| Error::OutputError("Invalid base URL".to_string()))?;
    let domain = url_parsed.host_str().ok_or_else(|| Error::OutputError("No host in URL".to_string()))?.to_string();

    Ok(Session {
        reqwest_client: client,
        base_url: input.base_url,
        domain,
        cookies: vec![],
    })
}

pub type LogoutInput<'a> = SessionPostInput<'a, Empty, Empty>;
pub type LogoutOutput = Empty;
pub async fn logout(input: LogoutInput<'_>) -> SessionPostOutput<LogoutOutput> {
    session_post(input, scope(), |_path| "/api/logout").await
}
