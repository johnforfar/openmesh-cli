use crate::sdk::{
    os::models::{OSChange, OSConfiguration},
    request::RequestIdResponse,
    utils::{
        Empty, SessionGetInput, SessionGetOutput, SessionPostInput, SessionPostOutput, session_get,
        session_post,
    },
};

pub fn scope() -> String {
    "/os".to_string()
}

pub type GetInput<'a> = SessionGetInput<'a, Empty, Empty>;
pub type GetOutput = OSConfiguration;
pub async fn get(input: GetInput<'_>) -> SessionGetOutput<GetOutput> {
    session_get(input, scope(), |_path| "/get").await
}

pub type SetInput<'a> = SessionPostInput<'a, Empty, OSChange>;
pub type SetOutput = RequestIdResponse;
pub async fn set(input: SetInput<'_>) -> SessionPostOutput<SetOutput> {
    session_post(input, scope(), |_path| "/set").await
}

pub type RebootInput<'a> = SessionPostInput<'a, Empty, Empty>;
pub type RebootOutput = RequestIdResponse;
pub async fn reboot(input: RebootInput<'_>) -> SessionPostOutput<RebootOutput> {
    session_post(input, scope(), |_path| "/reboot").await
}
