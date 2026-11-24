use std::env::{self, VarError};

use anyhow::{Context, Result, bail, ensure};
use chrono_tz::Tz;

pub(crate) struct Config {
    pub latitude: f64,
    pub longitude: f64,
    pub location_name: String,
    pub timezone: Tz,
}

pub(crate) struct SlackBot {
    token: String,
    channel_id: String,
}

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

impl SlackBot {
    pub(crate) fn from_env() -> Result<Self> {
        Self::parse(|name| env::var(name))
    }

    fn parse(read: impl Fn(&str) -> Result<String, VarError>) -> Result<Self> {
        Ok(Self {
            token: bot_token(&required(&read, "SLACK_BOT_TOKEN")?)?,
            channel_id: channel_id(&required(&read, "SLACK_CHANNEL_ID")?)?,
        })
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn channel_id(&self) -> &str {
        &self.channel_id
    }
}

fn bot_token(value: &str) -> Result<String> {
    let invalid = "SLACK_BOT_TOKEN にBot User OAuth Token（xoxb-...）を指定してください";
    let Some(rest) = value.strip_prefix("xoxb-") else {
        bail!("{invalid}");
    };
    ensure!(
        !rest.is_empty()
            && rest.len() <= 200
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "{invalid}"
    );
    Ok(value.to_owned())
}

fn channel_id(value: &str) -> Result<String> {
    let invalid = "SLACK_CHANNEL_ID にSlackのチャンネルID（CまたはGで始まる）を指定してください";
    ensure!(
        value.len() >= 9
            && value.len() <= 20
            && matches!(value.as_bytes().first(), Some(b'C' | b'G'))
            && value.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        "{invalid}"
    );
    Ok(value.to_owned())
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

    fn sample_bot_token() -> String {
        // Build at runtime so source scanners do not treat the fixture as a live token.
        ["xoxb", "0000000000", "unittestonlytoken"].join("-")
    }

    #[test]
    fn parses_slack_bot_token_and_channel() {
        let token = sample_bot_token();
        let bot = SlackBot::parse(|name| match name {
            "SLACK_BOT_TOKEN" => Ok(token.clone()),
            "SLACK_CHANNEL_ID" => Ok("C0123456789".into()),
            _ => Err(VarError::NotPresent),
        })
        .unwrap();
        assert_eq!(bot.token(), token);
        assert_eq!(bot.channel_id(), "C0123456789");
    }

    #[test]
    fn rejects_invalid_bot_token_without_echoing_it() {
        let result = bot_token(&["xoxp", "not", "a", "bot", "token"].join("-"));
        assert_eq!(
            result.err().unwrap().to_string(),
            "SLACK_BOT_TOKEN にBot User OAuth Token（xoxb-...）を指定してください"
        );
    }

    #[test]
    fn rejects_invalid_channel_id() {
        let result = channel_id("general");
        assert_eq!(
            result.err().unwrap().to_string(),
            "SLACK_CHANNEL_ID にSlackのチャンネルID（CまたはGで始まる）を指定してください"
        );
    }
}
