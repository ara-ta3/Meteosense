use std::env::{self, VarError};

use anyhow::{Context, Result, bail, ensure};
use chrono_tz::Tz;
use url::Url;

pub(crate) struct Config {
    pub latitude: f64,
    pub longitude: f64,
    pub location_name: String,
    pub timezone: Tz,
}

pub(crate) struct WebhookUrl(Url);

impl Config {
    pub(crate) fn from_env() -> Result<Self> {
        Self::parse(|name| env::var(name))
    }

    fn parse(read: impl Fn(&str) -> Result<String, VarError>) -> Result<Self> {
        let latitude = coordinate(&read, "METEOSENSE_LATITUDE", 90.0)?;
        let longitude = coordinate(&read, "METEOSENSE_LONGITUDE", 180.0)?;
        let location_name = required(&read, "METEOSENSE_LOCATION_NAME")?;
        ensure!(
            location_name.chars().count() <= 130 && !location_name.chars().any(char::is_control),
            "METEOSENSE_LOCATION_NAME は制御文字を含まない130文字以内の地点名を指定してください"
        );
        let timezone = match read("METEOSENSE_TIMEZONE") {
            Ok(value) => value.trim().parse().map_err(|_| {
                anyhow::anyhow!("METEOSENSE_TIMEZONE にIANAタイムゾーンを指定してください")
            })?,
            Err(VarError::NotPresent) => chrono_tz::Asia::Tokyo,
            Err(VarError::NotUnicode(_)) => bail!("METEOSENSE_TIMEZONE はUTF-8で指定してください"),
        };

        Ok(Self {
            latitude,
            longitude,
            location_name,
            timezone,
        })
    }
}

impl WebhookUrl {
    pub(crate) fn from_env() -> Result<Self> {
        Self::parse(&required(&|name| env::var(name), "SLACK_WEBHOOK_URL")?)
    }

    fn parse(value: &str) -> Result<Self> {
        let invalid = "SLACK_WEBHOOK_URL にSlack Incoming WebhookのHTTPS URLを指定してください";
        let url = Url::parse(value).map_err(|_| anyhow::anyhow!(invalid))?;
        ensure!(
            url.scheme() == "https"
                && matches!(
                    url.host_str(),
                    Some("hooks.slack.com" | "hooks.slack-gov.com")
                )
                && url.port().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "{invalid}"
        );

        let segments: Vec<_> = url.path().split('/').collect();
        ensure!(
            matches!(segments.as_slice(), ["", "services", team, channel, token]
            if [team, channel, token].into_iter().all(|segment| {
                !segment.is_empty() && segment.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
                })
            })),
            "{invalid}"
        );
        Ok(Self(url))
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

fn required(read: &impl Fn(&str) -> Result<String, VarError>, name: &str) -> Result<String> {
    let value = match read(name) {
        Ok(value) => value,
        Err(VarError::NotPresent) => bail!("{name} を設定してください"),
        Err(VarError::NotUnicode(_)) => bail!("{name} はUTF-8で指定してください"),
    };
    let value = value.trim();
    ensure!(!value.is_empty(), "{name} を設定してください");
    Ok(value.to_owned())
}

fn coordinate(
    read: &impl Fn(&str) -> Result<String, VarError>,
    name: &str,
    limit: f64,
) -> Result<f64> {
    let value = required(read, name)?
        .parse::<f64>()
        .with_context(|| format!("{name} は数値で指定してください"))?;
    ensure!(
        (-limit..=limit).contains(&value),
        "{name} は {}〜{limit} の範囲で指定してください",
        -limit
    );
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_env(name: &str) -> Result<String, VarError> {
        match name {
            "METEOSENSE_LATITUDE" => Ok("0".into()),
            "METEOSENSE_LONGITUDE" => Ok("180".into()),
            "METEOSENSE_LOCATION_NAME" => Ok("Sample Location".into()),
            _ => Err(VarError::NotPresent),
        }
    }

    #[test]
    fn parses_coordinates_and_defaults_timezone() {
        let config = Config::parse(sample_env).unwrap();
        assert_eq!(config.latitude, 0.0);
        assert_eq!(config.longitude, 180.0);
        assert_eq!(config.location_name, "Sample Location");
        assert_eq!(config.timezone, chrono_tz::Asia::Tokyo);
    }

    #[test]
    fn reports_missing_location_name() {
        let result = Config::parse(|name| match name {
            "METEOSENSE_LOCATION_NAME" => Err(VarError::NotPresent),
            _ => sample_env(name),
        });
        assert_eq!(
            result.err().unwrap().to_string(),
            "METEOSENSE_LOCATION_NAME を設定してください"
        );
    }

    #[test]
    fn rejects_latitudes_outside_range_and_non_finite_values() {
        for value in ["91", "NaN", "inf"] {
            let result = Config::parse(|name| match name {
                "METEOSENSE_LATITUDE" => Ok(value.into()),
                _ => sample_env(name),
            });
            assert_eq!(
                result.err().unwrap().to_string(),
                "METEOSENSE_LATITUDE は -90〜90 の範囲で指定してください"
            );
        }
    }

    #[test]
    fn rejects_unknown_timezone() {
        let result = Config::parse(|name| match name {
            "METEOSENSE_TIMEZONE" => Ok("Sample/Unknown".into()),
            _ => sample_env(name),
        });
        assert_eq!(
            result.err().unwrap().to_string(),
            "METEOSENSE_TIMEZONE にIANAタイムゾーンを指定してください"
        );
    }

    #[test]
    fn parses_slack_webhook_url() {
        let sample = "https://hooks.slack.com/services/SAMPLE/CHANNEL/TOKEN";
        assert_eq!(WebhookUrl::parse(sample).unwrap().as_str(), sample);
    }

    #[test]
    fn rejects_and_redacts_invalid_webhook() {
        let result = WebhookUrl::parse("https://example.com/services/SAMPLE/CHANNEL/SECRET");
        assert_eq!(
            result.err().unwrap().to_string(),
            "SLACK_WEBHOOK_URL にSlack Incoming WebhookのHTTPS URLを指定してください"
        );
    }
}
