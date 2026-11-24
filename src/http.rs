use std::time::Duration;

use anyhow::Error;
use ureq::{Agent, config::Config};

pub(crate) fn agent() -> Agent {
    Config::builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .max_redirects(0)
        .build()
        .into()
}

pub(crate) fn request_error(service: &str, error: ureq::Error) -> Error {
    match error {
        ureq::Error::StatusCode(status) => response_error(service, status),
        ureq::Error::Timeout(_) => anyhow::anyhow!("{service} への通信がタイムアウトしました"),
        _ => anyhow::anyhow!("{service} とのHTTP通信に失敗しました"),
    }
}

pub(crate) fn response_error(service: &str, status: u16) -> Error {
    anyhow::anyhow!("{service} がHTTP {status} を返しました")
}
