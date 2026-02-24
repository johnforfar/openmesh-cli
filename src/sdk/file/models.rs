use serde::{Deserialize, Serialize};

use crate::sdk::utils::Output;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ReadFile {
    pub path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WriteFile {
    pub path: String,
    pub content: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RemoveFile {
    pub path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ReadDirectory {
    pub path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CreateDirectory {
    pub path: String,
    pub make_parent: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RemoveDirectory {
    pub path: String,
    pub make_empty: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GetPermissions {
    pub path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SetPermissions {
    pub path: String,
    pub permissions: Vec<Permission>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct File {
    pub content: Output,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Directory {
    pub directories: Vec<String>,
    pub files: Vec<String>,
    pub symlinks: Vec<String>,
    pub unknown: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Entity {
    User(u32),
    Group(u32),
    Any,
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Permission {
    pub granted_to: Entity,
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}
