use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use ureq::Agent;

use crate::config::Config;
use crate::http::{request_error, response_error};
use crate::pressure::{HORIZON_HOURS, HourlyPressure, Pressure};

const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";
const SERVICE: &str = "Open-Meteo";
const MAX_ATTEMPTS: usize = 4;
const INITIAL_BACKOFF: Duration = Duration::from_secs(2);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);
const MAX_JITTER_MILLIS: u64 = 1_000;

enum FetchError {
    Retryable {
        error: anyhow::Error,
        retry_after: Option<Duration>,
    },
    Permanent(anyhow::Error),
}

pub(crate) fn fetch(
    agent: &Agent,
    config: &Config,
    reference: DateTime<Utc>,
) -> Result<HourlyPressure> {
    let horizon = TimeDelta::hours(HORIZON_HOURS as i64);
    let latitude = config.latitude.to_string();
    let longitude = config.longitude.to_string();
    let start = (reference - horizon).format("%Y-%m-%dT%H:%M").to_string();
    let end = (reference + horizon).format("%Y-%m-%dT%H:%M").to_string();

    retry_fetch(
        || fetch_once(agent, &latitude, &longitude, &start, &end),
        default_jitter,
        |attempt, error, delay| {
            eprintln!(
                "meteosense: {error:#}（{attempt}/{MAX_ATTEMPTS}回目）。{:.1}秒後に再試行します",
                delay.as_secs_f64()
            );
            thread::sleep(delay);
        },
    )
}

fn fetch_once(
    agent: &Agent,
    latitude: &str,
    longitude: &str,
    start: &str,
    end: &str,
) -> std::result::Result<HourlyPressure, FetchError> {
    let mut response = agent
        .get(FORECAST_URL)
        .query("latitude", latitude)
        .query("longitude", longitude)
        .query("cell_selection", "nearest")
        .query("hourly", "pressure_msl")
        .query("timezone", "GMT")
        .query("timeformat", "unixtime")
        .query("start_hour", start)
        .query("end_hour", end)
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(classify_transport_error)?;

    let status = response.status().as_u16();
    if !response.status().is_success() {
        let error = response_error(SERVICE, status);
        if is_retryable_status(status) {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| parse_retry_after(value, Utc::now()));
            return Err(FetchError::Retryable { error, retry_after });
        }
        return Err(FetchError::Permanent(error));
    }

    response
        .body_mut()
        .read_json::<ForecastResponse>()
        .map_err(|error| match error {
            ureq::Error::Json(_) => {
                FetchError::Permanent(anyhow::anyhow!("Open-Meteoの応答JSONを解釈できません"))
            }
            other => classify_transport_error(other),
        })?
        .into_hourly_pressure()
        .map_err(FetchError::Permanent)
}

fn retry_fetch<T>(
    mut attempt_fn: impl FnMut() -> std::result::Result<T, FetchError>,
    mut jitter: impl FnMut() -> Duration,
    mut on_retry: impl FnMut(usize, &anyhow::Error, Duration),
) -> Result<T> {
    for attempt in 1..=MAX_ATTEMPTS {
        match attempt_fn() {
            Ok(value) => return Ok(value),
            Err(FetchError::Permanent(error)) => return Err(error),
            Err(FetchError::Retryable { error, retry_after }) => {
                if attempt == MAX_ATTEMPTS {
                    return Err(error);
                }
                let delay = retry_delay(attempt, retry_after, jitter());
                on_retry(attempt, &error, delay);
            }
        }
    }
    unreachable!("MAX_ATTEMPTS is non-zero")
}

fn classify_transport_error(error: ureq::Error) -> FetchError {
    let retryable = matches!(
        error,
        ureq::Error::Timeout(_)
            | ureq::Error::Io(_)
            | ureq::Error::ConnectionFailed
            | ureq::Error::HostNotFound
    );
    let formatted = request_error(SERVICE, error);
    if retryable {
        FetchError::Retryable {
            error: formatted,
            retry_after: None,
        }
    } else {
        FetchError::Permanent(formatted)
    }
}

fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500..=599)
}

fn retry_delay(attempt: usize, retry_after: Option<Duration>, jitter: Duration) -> Duration {
    let shift = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
    let multiplier = 1_u32.checked_shl(shift).unwrap_or(u32::MAX);
    let backoff = INITIAL_BACKOFF.saturating_mul(multiplier);
    let base = retry_after.map_or(backoff, |hint| backoff.max(hint.min(MAX_RETRY_AFTER)));
    let bounded_jitter = jitter.min(Duration::from_millis(MAX_JITTER_MILLIS));
    base.saturating_add(bounded_jitter)
}

fn parse_retry_after(value: &str, now: DateTime<Utc>) -> Option<Duration> {
    let trimmed = value.trim();
    if let Ok(seconds) = trimmed.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let retry_at = DateTime::parse_from_rfc2822(trimmed)
        .ok()?
        .with_timezone(&Utc);
    let delta = retry_at.signed_duration_since(now);
    delta.to_std().ok().or(Some(Duration::ZERO))
}

fn default_jitter() -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.subsec_nanos());
    Duration::from_millis(u64::from(nanos) % MAX_JITTER_MILLIS)
}

#[derive(Deserialize)]
struct ForecastResponse {
    hourly_units: HourlyUnits,
    hourly: HourlyResponse,
}

#[derive(Deserialize)]
struct HourlyUnits {
    pressure_msl: String,
}

#[derive(Deserialize)]
struct HourlyResponse {
    time: Vec<i64>,
    pressure_msl: Vec<Option<f64>>,
}

impl ForecastResponse {
    fn into_hourly_pressure(self) -> Result<HourlyPressure> {
        ensure!(
            self.hourly_units.pressure_msl == "hPa",
            "Open-Meteoの気圧データの単位がhPaではありません"
        );

        let HourlyResponse { time, pressure_msl } = self.hourly;
        ensure!(
            time.len() == pressure_msl.len(),
            "Open-Meteoの時刻と気圧の件数が一致しません"
        );
        ensure!(
            time.windows(2).all(|pair| pair[0] < pair[1]),
            "Open-Meteoの時刻が昇順に並んでいません"
        );

        time.into_iter()
            .zip(pressure_msl)
            .filter_map(|(timestamp, pressure)| pressure.map(|value| (timestamp, value)))
            .map(|(timestamp, value)| {
                let time = DateTime::from_timestamp(timestamp, 0)
                    .context("Open-Meteoの時刻が範囲外です")?;
                let pressure = Pressure::from_hpa(value)
                    .with_context(|| format!("Open-Meteoの気圧データが不正です（{time}）"))?;
                Ok((time, pressure))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn decodes_hourly_response_without_shifting_values_after_null() {
        let payload = r#"{
            "hourly_units": {"time": "unixtime", "pressure_msl": "hPa"},
            "hourly": {"time": [1767312000, 1767315600, 1767319200], "pressure_msl": [1013.2, null, 1012.4]}
        }"#;
        let response: ForecastResponse = serde_json::from_str(payload).unwrap();

        assert_eq!(
            response.into_hourly_pressure().unwrap(),
            HourlyPressure::from([
                (
                    Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
                    Pressure::from_hpa(1013.2).unwrap()
                ),
                (
                    Utc.with_ymd_and_hms(2026, 1, 2, 2, 0, 0).unwrap(),
                    Pressure::from_hpa(1012.4).unwrap()
                ),
            ])
        );
    }

    #[test]
    fn rejects_mismatched_array_lengths() {
        let response: ForecastResponse = serde_json::from_str(
            r#"{
            "hourly_units": {"pressure_msl": "hPa"},
            "hourly": {"time": [1767312000], "pressure_msl": []}
        }"#,
        )
        .unwrap();

        assert_eq!(
            response.into_hourly_pressure().unwrap_err().to_string(),
            "Open-Meteoの時刻と気圧の件数が一致しません"
        );
    }

    #[test]
    fn retries_transient_failures_and_returns_first_success() {
        let mut outcomes = vec![
            Err(FetchError::Retryable {
                error: anyhow::anyhow!("Open-Meteo への通信がタイムアウトしました（connect）"),
                retry_after: None,
            }),
            Err(FetchError::Retryable {
                error: anyhow::anyhow!("Open-Meteo がHTTP 429 を返しました"),
                retry_after: Some(Duration::from_secs(6)),
            }),
            Ok(42),
        ]
        .into_iter();
        let mut recorded = Vec::new();

        let value = retry_fetch(
            || outcomes.next().unwrap(),
            || Duration::from_millis(250),
            |attempt, error, delay| recorded.push((attempt, error.to_string(), delay)),
        )
        .unwrap();

        assert_eq!(value, 42);
        assert_eq!(
            recorded,
            vec![
                (
                    1,
                    "Open-Meteo への通信がタイムアウトしました（connect）".into(),
                    Duration::from_millis(2_250)
                ),
                (
                    2,
                    "Open-Meteo がHTTP 429 を返しました".into(),
                    Duration::from_millis(6_250)
                ),
            ]
        );
    }

    #[test]
    fn stops_immediately_on_permanent_error() {
        let mut attempts = 0;
        let mut retried = false;

        let err = retry_fetch::<()>(
            || {
                attempts += 1;
                Err(FetchError::Permanent(anyhow::anyhow!(
                    "Open-Meteo がHTTP 400 を返しました"
                )))
            },
            || Duration::ZERO,
            |_, _, _| retried = true,
        )
        .unwrap_err();

        assert_eq!(attempts, 1);
        assert!(!retried);
        assert_eq!(err.to_string(), "Open-Meteo がHTTP 400 を返しました");
    }

    #[test]
    fn returns_last_error_after_exhausting_attempts_with_exponential_backoff() {
        let mut attempt_no = 0;
        let mut delays = Vec::new();

        let err = retry_fetch::<()>(
            || {
                attempt_no += 1;
                Err(FetchError::Retryable {
                    error: anyhow::anyhow!("失敗 #{attempt_no}"),
                    retry_after: None,
                })
            },
            || Duration::from_millis(100),
            |_, _, delay| delays.push(delay),
        )
        .unwrap_err();

        assert_eq!(attempt_no, MAX_ATTEMPTS);
        assert_eq!(
            delays,
            vec![
                Duration::from_millis(2_100),
                Duration::from_millis(4_100),
                Duration::from_millis(8_100),
            ]
        );
        assert_eq!(err.to_string(), "失敗 #4");
    }

    #[test]
    fn respects_and_caps_retry_after_header() {
        let now = Utc.with_ymd_and_hms(2026, 11, 27, 1, 0, 0).unwrap();
        assert_eq!(
            parse_retry_after(" 12 ", now),
            Some(Duration::from_secs(12))
        );
        assert_eq!(
            parse_retry_after("Fri, 27 Nov 2026 01:00:25 GMT", now),
            Some(Duration::from_secs(25))
        );
        assert_eq!(
            parse_retry_after("Fri, 27 Nov 2026 00:59:50 GMT", now),
            Some(Duration::ZERO)
        );
        assert_eq!(parse_retry_after("not-a-date", now), None);

        // Retry-After larger than backoff takes precedence up to MAX_RETRY_AFTER.
        assert_eq!(
            retry_delay(1, Some(Duration::from_secs(10)), Duration::ZERO),
            Duration::from_secs(10)
        );
        assert_eq!(
            retry_delay(
                1,
                Some(Duration::from_secs(120)),
                Duration::from_millis(500)
            ),
            Duration::from_millis(30_500)
        );
        // Retry-After smaller than exponential backoff keeps backoff floor.
        assert_eq!(
            retry_delay(2, Some(Duration::from_secs(1)), Duration::ZERO),
            Duration::from_secs(4)
        );
    }

    #[test]
    fn classifies_retryable_and_permanent_errors() {
        for status in [408, 429, 500, 502, 503, 504] {
            assert!(is_retryable_status(status), "HTTP {status} should retry");
        }
        for status in [200, 301, 400, 401, 403, 404] {
            assert!(
                !is_retryable_status(status),
                "HTTP {status} should not retry"
            );
        }

        assert!(matches!(
            classify_transport_error(ureq::Error::Timeout(ureq::Timeout::Connect)),
            FetchError::Retryable { error, retry_after: None }
                if error.to_string() == "Open-Meteo への通信がタイムアウトしました（connect）"
        ));
        assert!(matches!(
            classify_transport_error(ureq::Error::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "reset"
            ))),
            FetchError::Retryable { .. }
        ));
        assert!(matches!(
            classify_transport_error(ureq::Error::ConnectionFailed),
            FetchError::Retryable { .. }
        ));
        assert!(matches!(
            classify_transport_error(ureq::Error::HostNotFound),
            FetchError::Retryable { .. }
        ));
        assert!(matches!(
            classify_transport_error(ureq::Error::TooManyRedirects),
            FetchError::Permanent(_)
        ));
    }
}
