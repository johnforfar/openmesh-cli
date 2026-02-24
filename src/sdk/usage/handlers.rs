use crate::sdk::{
    usage::models::{CpuUsage, DiskUsage, MemoryUsage},
    utils::{Empty, SessionGetInput, SessionGetOutput, session_get},
};

pub fn scope() -> String {
    "/usage".to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct CpuPath {
    pub scope: String,
}
pub type CpuInput<'a> = SessionGetInput<'a, CpuPath, Empty>;
pub type CpuOutput = Vec<CpuUsage>;
pub async fn cpu(input: CpuInput<'_>) -> SessionGetOutput<CpuOutput> {
    session_get(input, scope(), |path| {
        format!("/{scope}/cpu", scope = path.scope)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryPath {
    pub scope: String,
}
pub type MemoryInput<'a> = SessionGetInput<'a, MemoryPath, Empty>;
pub type MemoryOutput = MemoryUsage;
pub async fn memory(input: MemoryInput<'_>) -> SessionGetOutput<MemoryOutput> {
    session_get(input, scope(), |path| {
        format!("/{scope}/memory", scope = path.scope)
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiskPath {
    pub scope: String,
}
pub type DiskInput<'a> = SessionGetInput<'a, DiskPath, Empty>;
pub type DiskOutput = Vec<DiskUsage>;
pub async fn disk(input: DiskInput<'_>) -> SessionGetOutput<DiskOutput> {
    session_get(input, scope(), |path| {
        format!("/{scope}/disk", scope = path.scope)
    })
    .await
}
