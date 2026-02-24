use crate::sdk::{
    info::{
        Group, User,
        models::{Flake, FlakeQuery},
    },
    utils::{Empty, SessionGetInput, SessionGetOutput, session_get},
};

pub fn scope() -> String {
    "/info".to_string()
}

pub type FlakeInput<'a> = SessionGetInput<'a, Empty, FlakeQuery>;
pub type FlakeOutput = Flake;
pub async fn flake(input: FlakeInput<'_>) -> SessionGetOutput<FlakeOutput> {
    session_get(input, scope(), |_path| "/flake").await
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsersPath {
    pub scope: String,
}
pub type UsersInput<'a> = SessionGetInput<'a, UsersPath, Empty>;
pub type UsersOutput = Vec<User>;
pub async fn users(input: UsersInput<'_>) -> SessionGetOutput<UsersOutput> {
    session_get(input, scope(), |path| {
        format!("/users/{scope}/users", scope = path.scope)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupsPath {
    pub scope: String,
}
pub type GroupsInput<'a> = SessionGetInput<'a, GroupsPath, Empty>;
pub type GroupsOutput = Vec<Group>;
pub async fn groups(input: GroupsInput<'_>) -> SessionGetOutput<GroupsOutput> {
    session_get(input, scope(), |path| {
        format!("/users/{scope}/groups", scope = path.scope)
    })
    .await
}
