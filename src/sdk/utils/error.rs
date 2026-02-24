use std::fmt;

#[derive(Debug)]
pub enum Error {
    XnodeManagerSDKError(XnodeManagerSDKError),
    ReqwestError(reqwest::Error),
    SerdeJsonError(serde_json::Error),
    OutputError(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::XnodeManagerSDKError(_) => write!(f, "XnodeManagerSDKError"),
            Error::ReqwestError(e) => write!(f, "ReqwestError: {}", e),
            Error::SerdeJsonError(e) => write!(f, "SerdeJsonError: {}", e),
            Error::OutputError(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug)]
pub struct XnodeManagerSDKError {}
