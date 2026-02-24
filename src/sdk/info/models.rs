use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FlakeQuery {
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Flake {
    #[serde(rename = "lastModified")]
    pub last_modified: Option<u64>,
    pub revision: Option<String>,
    pub hostname: Option<String>,
    #[serde(rename = "stateVersion")]
    pub state_version: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct User {
    pub name: String,
    pub id: u32,
    pub group: u32,
    pub description: String,
    pub home: String,
    pub login: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Group {
    pub name: String,
    pub id: u32,
    pub members: Vec<String>,
}
