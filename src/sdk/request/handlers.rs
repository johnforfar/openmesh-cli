use crate::sdk::{
    request::{CommandInfo, RequestInfo, models::RequestId},
    utils::{Empty, SessionGetInput, SessionGetOutput, session_get},
};

pub fn scope() -> String {
    "/request".to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct RequestInfoPath {
    pub request_id: RequestId,
}
pub type RequestInfoInput<'a> = SessionGetInput<'a, RequestInfoPath, Empty>;
pub type RequestInfoOutput = RequestInfo;
pub async fn request_info(input: RequestInfoInput<'_>) -> SessionGetOutput<RequestInfoOutput> {
    session_get(input, scope(), |path| {
        format!("/{request_id}/info", request_id = path.request_id)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandInfoPath {
    pub request_id: RequestId,
    pub command: String,
}
pub type CommandInfoInput<'a> = SessionGetInput<'a, CommandInfoPath, Empty>;
pub type CommandInfoOutput = CommandInfo;
pub async fn command_info(input: CommandInfoInput<'_>) -> SessionGetOutput<CommandInfoOutput> {
    session_get(input, scope(), |path| {
        format!(
            "/{request_id}/command/{command}/info",
            request_id = path.request_id,
            command = path.command
        )
    })
    .await
}
