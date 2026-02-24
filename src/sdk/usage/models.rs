use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CpuUsage {
    pub name: String,
    pub used: f32,
    pub frequency: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MemoryUsage {
    pub used: u64,
    pub total: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DiskUsage {
    pub mount_point: String,
    pub used: u64,
    pub total: u64,
}
