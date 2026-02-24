use crate::sdk::{
    process::models::{Log, LogQuery, Process, ProcessCommand},
    request::RequestIdResponse,
    utils::{
        Empty, SessionGetInput, SessionGetOutput, SessionPostInput, SessionPostOutput, session_get,
        session_post,
    },
};

pub fn scope() -> String {
    "/process".to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListPath {
    pub scope: String,
}
pub type ListInput<'a> = SessionGetInput<'a, ListPath, Empty>;
pub type ListOutput = Vec<Process>;
pub async fn list(input: ListInput<'_>) -> SessionGetOutput<ListOutput> {
    session_get(input, scope(), |path| {
        format!("/{scope}/list", scope = path.scope)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogsPath {
    pub scope: String,
    pub process: String,
}
pub type LogsInput<'a> = SessionGetInput<'a, LogsPath, LogQuery>;
pub type LogsOutput = Vec<Log>;
pub async fn logs(input: LogsInput<'_>) -> SessionGetOutput<LogsOutput> {
    session_get(input, scope(), |path| {
        format!(
            "/{scope}/{process}/logs",
            scope = path.scope,
            process = path.process
        )
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutePath {
    pub scope: String,
    pub process: String,
}
pub type ExecuteInput<'a> = SessionPostInput<'a, ExecutePath, ProcessCommand>;
pub type ExecuteOutput = RequestIdResponse;
pub async fn execute(input: ExecuteInput<'_>) -> SessionPostOutput<ExecuteOutput> {
    session_post(input, scope(), |path| {
        format!(
            "/{scope}/{process}/execute",
            scope = path.scope,
            process = path.process
        )
    })
    .await
}
