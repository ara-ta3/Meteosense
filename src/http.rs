use std::time::Duration;

use anyhow::Error;
use ureq::{Agent, config::Config};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn agent() -> Agent {
    Config::builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(GLOBAL_TIMEOUT))
        .max_redirects(0)
        .build()
        .into()
}

pub(crate) fn request_error(service: &str, error: ureq::Error) -> Error {
    match error {
        ureq::Error::StatusCode(status) => response_error(service, status),
        ureq::Error::Timeout(kind) => {
            anyhow::anyhow!("{service} への通信がタイムアウトしました（{kind}）")
        }
        _ => anyhow::anyhow!("{service} とのHTTP通信に失敗しました"),
    }
}

pub(crate) fn response_error(service: &str, status: u16) -> Error {
    anyhow::anyhow!("{service} がHTTP {status} を返しました")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use ureq::Timeout;

    #[test]
    fn includes_timeout_kind_in_error_message() {
        assert_eq!(
            request_error("Open-Meteo", ureq::Error::Timeout(Timeout::Connect)).to_string(),
            "Open-Meteo への通信がタイムアウトしました（connect）"
        );
        assert_eq!(
            request_error("Open-Meteo", ureq::Error::Timeout(Timeout::Global)).to_string(),
            "Open-Meteo への通信がタイムアウトしました（global）"
        );
    }

    #[test]
    fn formats_status_and_generic_transport_errors() {
        assert_eq!(
            request_error("Open-Meteo", ureq::Error::StatusCode(429)).to_string(),
            "Open-Meteo がHTTP 429 を返しました"
        );
        let io_err = io::Error::new(io::ErrorKind::ConnectionReset, "reset");
        assert_eq!(
            request_error("Open-Meteo", ureq::Error::Io(io_err)).to_string(),
            "Open-Meteo とのHTTP通信に失敗しました"
        );
    }
}
