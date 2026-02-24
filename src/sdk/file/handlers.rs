use crate::sdk::{
    file::{
        GetPermissions, Permission, SetPermissions,
        models::{
            CreateDirectory, Directory, File, ReadDirectory, ReadFile, RemoveDirectory, RemoveFile,
            WriteFile,
        },
    },
    utils::{
        Empty, SessionGetInput, SessionGetOutput, SessionPostInput, SessionPostOutput, session_get,
        session_post,
    },
};

pub fn scope() -> String {
    "/file".to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadFilePath {
    pub scope: String,
}
pub type ReadFileInput<'a> = SessionGetInput<'a, ReadFilePath, ReadFile>;
pub type ReadFileOutput = File;
pub async fn read_file(input: ReadFileInput<'_>) -> SessionGetOutput<ReadFileOutput> {
    session_get(input, scope(), |path| {
        format!("/{scope}/read_file", scope = path.scope)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct WriteFilePath {
    pub scope: String,
}
pub type WriteFileInput<'a> = SessionPostInput<'a, WriteFilePath, WriteFile>;
pub type WriteFileOutput = Empty;
pub async fn write_file(input: WriteFileInput<'_>) -> SessionPostOutput<WriteFileOutput> {
    session_post(input, scope(), |path| {
        format!("/{scope}/write_file", scope = path.scope)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoveFilePath {
    pub scope: String,
}
pub type RemoveFileInput<'a> = SessionPostInput<'a, RemoveFilePath, RemoveFile>;
pub type RemoveFileOutput = Empty;
pub async fn remove_file(input: RemoveFileInput<'_>) -> SessionPostOutput<RemoveFileOutput> {
    session_post(input, scope(), |path| {
        format!("/{scope}/remove_file", scope = path.scope)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadDirectoryPath {
    pub scope: String,
}
pub type ReadDirectoryInput<'a> = SessionGetInput<'a, ReadDirectoryPath, ReadDirectory>;
pub type ReadDirectoryOutput = Directory;
pub async fn read_directory(
    input: ReadDirectoryInput<'_>,
) -> SessionGetOutput<ReadDirectoryOutput> {
    session_get(input, scope(), |path| {
        format!("/{scope}/read_directory", scope = path.scope)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateDirectoryPath {
    pub scope: String,
}
pub type CreateDirectoryInput<'a> = SessionPostInput<'a, CreateDirectoryPath, CreateDirectory>;
pub type CreateDirectoryOutput = Empty;
pub async fn create_directory(
    input: CreateDirectoryInput<'_>,
) -> SessionPostOutput<CreateDirectoryOutput> {
    session_post(input, scope(), |path| {
        format!("/{scope}/create_directory", scope = path.scope)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoveDirectoryPath {
    pub scope: String,
}
pub type RemoveDirectoryInput<'a> = SessionPostInput<'a, RemoveDirectoryPath, RemoveDirectory>;
pub type RemoveDirectoryOutput = Empty;
pub async fn remove_directory(
    input: RemoveDirectoryInput<'_>,
) -> SessionPostOutput<RemoveDirectoryOutput> {
    session_post(input, scope(), |path| {
        format!("/{scope}/remove_directory", scope = path.scope)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetPermissionsPath {
    pub scope: String,
}
pub type GetPermissionsInput<'a> = SessionGetInput<'a, GetPermissionsPath, GetPermissions>;
pub type GetPermissionsOutput = Vec<Permission>;
pub async fn get_permissions(
    input: GetPermissionsInput<'_>,
) -> SessionGetOutput<GetPermissionsOutput> {
    session_get(input, scope(), |path| {
        format!("/{scope}/get_permissions", scope = path.scope)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetPermissionsPath {
    pub scope: String,
}
pub type SetPermissionsInput<'a> = SessionPostInput<'a, SetPermissionsPath, SetPermissions>;
pub type SetPermissionsOutput = Empty;
pub async fn set_permissions(
    input: SetPermissionsInput<'_>,
) -> SessionPostOutput<SetPermissionsOutput> {
    session_post(input, scope(), |path| {
        format!("/{scope}/set_permissions", scope = path.scope)
    })
    .await
}
