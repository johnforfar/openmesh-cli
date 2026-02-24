use serde::{Deserialize, Serialize};

use crate::sdk::utils::Output;

pub type RequestId = u32;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RequestIdResponse {
    pub request_id: RequestId,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum RequestIdResult {
    Success { body: Option<String> },
    Error { error: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RequestInfo {
    pub commands: Vec<String>,
    pub result: Option<RequestIdResult>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CommandInfo {
    pub command: String,
    pub stdout: Output,
    pub stderr: Output,
    pub result: Option<String>,
}
