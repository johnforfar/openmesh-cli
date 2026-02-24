use crate::sdk::{
    config::models::{ContainerChange, ContainerConfiguration},
    request::RequestIdResponse,
    utils::{
        Empty, SessionGetInput, SessionGetOutput, SessionPostInput, SessionPostOutput, session_get,
        session_post,
    },
};

pub fn scope() -> String {
    "/config".to_string()
}

pub type ContainersInput<'a> = SessionGetInput<'a, Empty, Empty>;
pub type ContainersOutput = Vec<String>;
pub async fn containers(input: ContainersInput<'_>) -> SessionGetOutput<ContainersOutput> {
    session_get(input, scope(), |_path| "/containers").await
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetPath {
    pub container: String,
}
pub type GetInput<'a> = SessionGetInput<'a, GetPath, Empty>;
pub type GetOutput = ContainerConfiguration;
pub async fn get(input: GetInput<'_>) -> SessionGetOutput<GetOutput> {
    session_get(input, scope(), |path| {
        format!("/container/{container}/get", container = path.container)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetPath {
    pub container: String,
}
pub type SetInput<'a> = SessionPostInput<'a, SetPath, ContainerChange>;
pub type SetOutput = RequestIdResponse;
pub async fn set(input: SetInput<'_>) -> SessionPostOutput<SetOutput> {
    session_post(input, scope(), |path| {
        format!("/container/{container}/set", container = path.container)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemovePath {
    pub container: String,
}
pub type RemoveInput<'a> = SessionPostInput<'a, RemovePath, Empty>;
pub type RemoveOutput = RequestIdResponse;
pub async fn remove(input: RemoveInput<'_>) -> SessionPostOutput<RemoveOutput> {
    session_post(input, scope(), |path| {
        format!("/container/{container}/remove", container = path.container)
    })
    .await
}
