use anyhow::{Context, Result, ensure};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use ureq::Agent;

use crate::config::Config;
use crate::http::{request_error, response_error};
use crate::pressure::{HORIZON_HOURS, HourlyPressure, Pressure};

const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";
const SERVICE: &str = "Open-Meteo";

pub(crate) fn fetch(
    agent: &Agent,
    config: &Config,
    reference: DateTime<Utc>,
) -> Result<HourlyPressure> {
    let horizon = TimeDelta::hours(HORIZON_HOURS as i64);
    let start = (reference - horizon).format("%Y-%m-%dT%H:%M").to_string();
    let end = (reference + horizon).format("%Y-%m-%dT%H:%M").to_string();
    let mut response = agent
        .get(FORECAST_URL)
        .query("latitude", config.latitude.to_string())
        .query("longitude", config.longitude.to_string())
        .query("cell_selection", "nearest")
        .query("hourly", "pressure_msl")
        .query("timezone", "GMT")
        .query("timeformat", "unixtime")
        .query("start_hour", &start)
        .query("end_hour", &end)
        .call()
        .map_err(|error| request_error(SERVICE, error))?;
    if !response.status().is_success() {
        return Err(response_error(SERVICE, response.status().as_u16()));
    }
    response
        .body_mut()
        .read_json::<ForecastResponse>()
        .map_err(|error| match error {
            ureq::Error::Json(_) => anyhow::anyhow!("Open-Meteoの応答JSONを解釈できません"),
            _ => request_error(SERVICE, error),
        })?
        .into_hourly_pressure()
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
}
