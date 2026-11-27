use std::fmt::Write as _;

use anyhow::{Result, bail, ensure};
use chrono::{DateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{Value, json};
use ureq::Agent;
use url::Url;

use crate::config::SlackBot;
use crate::http::{request_error, response_error};
use crate::pressure::{AlertLevel, Analysis, IntervalChange, WindowChange};

const SERVICE: &str = "Slack";
const ATTRIBUTION: &str = "気象データ: Open-Meteo";
const ATTRIBUTION_LINK: &str = "気象データ: <https://open-meteo.com/|Open-Meteo>";
const CHART_FILENAME: &str = "pressure.png";
const CHART_TITLE: &str = "前後24時間の気圧";
const CHART_ALT_TEXT: &str = "前後24時間の気圧グラフ";
const GET_UPLOAD_URL: &str = "https://slack.com/api/files.getUploadURLExternal";
const COMPLETE_UPLOAD: &str = "https://slack.com/api/files.completeUploadExternal";

pub(crate) struct Report {
    title: String,
    reference_local: String,
    alert: AlertLevel,
    current_hpa: f64,
    past_lines: String,
    future_lines: String,
    max_drop: String,
    max_rise: String,
    large_rise: bool,
}

impl Report {
    pub(crate) fn new(analysis: &Analysis, location_name: &str, timezone: Tz) -> Self {
        Self {
            title: format!("Meteosense | {location_name}"),
            reference_local: local_time(analysis.reference, timezone),
            alert: analysis.alert,
            current_hpa: analysis.current.rounded_hpa(),
            past_lines: interval_lines(&analysis.past, analysis.reference, timezone),
            future_lines: interval_lines(&analysis.future, analysis.reference, timezone),
            max_drop: window_text(analysis.max_drop.as_ref(), timezone),
            max_rise: window_text(analysis.max_rise.as_ref(), timezone),
            large_rise: analysis
                .max_rise
                .as_ref()
                .is_some_and(WindowChange::is_large_rise),
        }
    }

    pub(crate) fn text(&self) -> String {
        let mut text = format!(
            "{}\n\n{}\n警戒レベル（気圧変化）: {}\n現在（0h）の推定海面更正気圧: {:.1} hPa\n\n\
             過去の気圧変化（モデル値）\n差分 = 現在の気圧 − 各時点の気圧\n{}\n\n\
             今後の気圧変化（予報）\n差分 = 各時点の気圧 − 現在の気圧\n{}\n\n\
             今後24時間の最大6時間低下量\n{}\n\n今後24時間の最大6時間上昇量\n{}",
            self.title,
            self.reference_local,
            self.alert.label(),
            self.current_hpa,
            self.past_lines,
            self.future_lines,
            self.max_drop,
            self.max_rise,
        );
        if self.large_rise {
            text.push_str("\n大きな気圧上昇を予測");
        }
        write!(text, "\n\n{ATTRIBUTION}").expect("Stringへの書き込みは失敗しない");
        text
    }

    fn blocks(&self) -> Vec<Value> {
        let alert = self.alert.label();
        let mut rise_note = String::new();
        if self.large_rise {
            rise_note.push_str("\n*大きな気圧上昇を予測*");
        }
        vec![
            json!({
                "type": "header",
                "text": {"type": "plain_text", "text": self.title, "emoji": true}
            }),
            json!({
                "type": "section",
                "fields": [
                    {
                        "type": "mrkdwn",
                        "text": format!("*基準時刻*\n{}", self.reference_local)
                    },
                    {
                        "type": "mrkdwn",
                        "text": format!("*警戒レベル*\n*{alert}*")
                    },
                    {
                        "type": "mrkdwn",
                        "text": format!("*現在の気圧（0h）*\n`{:.1}` hPa", self.current_hpa)
                    },
                    {
                        "type": "mrkdwn",
                        "text": format!("*最大6時間低下*\n{}", self.max_drop)
                    }
                ]
            }),
            json!({"type": "divider"}),
            json!({
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": format!(
                        "*過去の気圧変化（モデル値）*\n差分 = 現在の気圧 − 各時点の気圧\n```{}```",
                        self.past_lines
                    )
                }
            }),
            json!({
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": format!(
                        "*今後の気圧変化（予報）*\n差分 = 各時点の気圧 − 現在の気圧\n```{}```",
                        self.future_lines
                    )
                }
            }),
            json!({"type": "divider"}),
            json!({
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": format!(
                        "*今後24時間の最大6時間変化*\n低下: {}\n上昇: {}{rise_note}",
                        self.max_drop,
                        self.max_rise
                    )
                }
            }),
            json!({
                "type": "context",
                "elements": [{"type": "mrkdwn", "text": ATTRIBUTION_LINK}]
            }),
        ]
    }

    fn complete_upload_body(&self, channel_id: &str, file_id: &str) -> Value {
        json!({
            "channel_id": channel_id,
            "files": [{"id": file_id, "title": CHART_TITLE}],
            "blocks": self.blocks(),
        })
    }
}

pub(crate) fn post(agent: &Agent, bot: &SlackBot, report: &Report, chart_png: &[u8]) -> Result<()> {
    ensure!(!chart_png.is_empty(), "Slackへ投稿する気圧グラフが空です");
    let (upload_url, file_id) = request_upload_url(agent, bot, chart_png.len())?;
    upload_bytes(agent, &upload_url, chart_png)?;
    complete_upload(agent, bot, report, bot.channel_id(), &file_id)?;
    Ok(())
}

fn request_upload_url(agent: &Agent, bot: &SlackBot, length: usize) -> Result<(Url, String)> {
    let mut response = agent
        .post(GET_UPLOAD_URL)
        .header("Authorization", format!("Bearer {}", bot.token()))
        .send_form([
            ("filename", CHART_FILENAME),
            ("length", &length.to_string()),
            ("alt_txt", CHART_ALT_TEXT),
        ])
        .map_err(|error| request_error(SERVICE, error))?;
    let body: UploadUrlResponse = read_api_json(&mut response)?;
    ensure_ok(body.ok, body.error.as_deref())?;
    let upload_url = body
        .upload_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("SlackからアップロードURLを受け取れませんでした"))?;
    let file_id = body
        .file_id
        .ok_or_else(|| anyhow::anyhow!("SlackからファイルIDを受け取れませんでした"))?;
    Ok((validated_upload_url(upload_url)?, file_id))
}

fn upload_bytes(agent: &Agent, upload_url: &Url, chart_png: &[u8]) -> Result<()> {
    let response = agent
        .post(upload_url.as_str())
        .header("Content-Type", "application/octet-stream")
        .send(chart_png)
        .map_err(|error| request_error(SERVICE, error))?;
    if !response.status().is_success() {
        return Err(response_error(SERVICE, response.status().as_u16()));
    }
    Ok(())
}

fn complete_upload(
    agent: &Agent,
    bot: &SlackBot,
    report: &Report,
    channel_id: &str,
    file_id: &str,
) -> Result<()> {
    let mut response = agent
        .post(COMPLETE_UPLOAD)
        .header("Authorization", format!("Bearer {}", bot.token()))
        .send_json(report.complete_upload_body(channel_id, file_id))
        .map_err(|error| request_error(SERVICE, error))?;
    let body: ApiStatus = read_api_json(&mut response)?;
    ensure_ok(body.ok, body.error.as_deref())
}

fn validated_upload_url(value: &str) -> Result<Url> {
    let url = Url::parse(value)
        .map_err(|_| anyhow::anyhow!("Slackから想定外のアップロード先を受け取りました"))?;
    ensure!(
        url.scheme() == "https"
            && matches!(
                url.host_str(),
                Some("files.slack.com" | "files.slack-gov.com")
            )
            && url.username().is_empty()
            && url.password().is_none(),
        "Slackから想定外のアップロード先を受け取りました"
    );
    Ok(url)
}

fn read_api_json<T: for<'de> Deserialize<'de>>(
    response: &mut ureq::http::Response<ureq::Body>,
) -> Result<T> {
    if !response.status().is_success() {
        return Err(response_error(SERVICE, response.status().as_u16()));
    }
    response
        .body_mut()
        .read_json()
        .map_err(|error| request_error(SERVICE, error))
}

fn ensure_ok(ok: bool, error: Option<&str>) -> Result<()> {
    if ok {
        return Ok(());
    }
    match error {
        Some(code)
            if !code.is_empty()
                && code.len() <= 64
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') =>
        {
            bail!("Slack APIがエラーを返しました（{code}）")
        }
        _ => bail!("Slack APIがエラーを返しました"),
    }
}

fn interval_lines(intervals: &[IntervalChange], reference: DateTime<Utc>, timezone: Tz) -> String {
    let mut text = String::new();
    for (index, interval) in intervals.iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let time = reference + TimeDelta::hours(interval.offset_hours);
        write!(
            text,
            "{:+}h（{}）: {:.1} hPa（差分 {:+.1} hPa）",
            interval.offset_hours,
            local_time(time, timezone),
            interval.pressure.rounded_hpa(),
            interval.change.hpa()
        )
        .expect("Stringへの書き込みは失敗しない");
    }
    text
}

fn window_text(window: Option<&WindowChange>, timezone: Tz) -> String {
    match window {
        Some(window) => format!(
            "{:+.1} hPa（{} 〜 {}）",
            window.change.hpa(),
            local_time(window.start, timezone),
            local_time(window.end, timezone)
        ),
        None => "該当なし".into(),
    }
}

fn local_time(time: DateTime<Utc>, timezone: Tz) -> String {
    time.with_timezone(&timezone)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

#[derive(Deserialize)]
struct UploadUrlResponse {
    ok: bool,
    error: Option<String>,
    upload_url: Option<String>,
    file_id: Option<String>,
}

#[derive(Deserialize)]
struct ApiStatus {
    ok: bool,
    error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pressure::{HourlyPressure, Pressure, analyze};
    use chrono::TimeZone;

    fn sample_report() -> Report {
        let reference = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
        let hourly: HourlyPressure = (-24..=24)
            .map(|offset| {
                let hpa = if offset <= 6 {
                    1013.2 - offset as f64
                } else {
                    1007.2 + (offset - 6) as f64
                };
                (
                    reference + TimeDelta::hours(offset),
                    Pressure::from_hpa(hpa).unwrap(),
                )
            })
            .collect();
        let analysis = analyze(&hourly, reference).unwrap();
        Report::new(&analysis, "Sample Location", chrono_tz::Asia::Tokyo)
    }

    #[test]
    fn generates_rich_block_kit_complete_upload_body() {
        let report = sample_report();
        let past = "-24h（2025-12-31 18:00）: 1037.2 hPa（差分 -24.0 hPa）\n\
                    -12h（2026-01-01 06:00）: 1025.2 hPa（差分 -12.0 hPa）\n\
                    -6h（2026-01-01 12:00）: 1019.2 hPa（差分 -6.0 hPa）\n\
                    -3h（2026-01-01 15:00）: 1016.2 hPa（差分 -3.0 hPa）\n\
                    -1h（2026-01-01 17:00）: 1014.2 hPa（差分 -1.0 hPa）";
        let future = "+1h（2026-01-01 19:00）: 1012.2 hPa（差分 -1.0 hPa）\n\
                      +3h（2026-01-01 21:00）: 1010.2 hPa（差分 -3.0 hPa）\n\
                      +6h（2026-01-02 00:00）: 1007.2 hPa（差分 -6.0 hPa）\n\
                      +12h（2026-01-02 06:00）: 1013.2 hPa（差分 +0.0 hPa）\n\
                      +24h（2026-01-02 18:00）: 1025.2 hPa（差分 +12.0 hPa）";
        let drop = "-6.0 hPa（2026-01-01 18:00 〜 2026-01-02 00:00）";
        let rise = "+6.0 hPa（2026-01-02 00:00 〜 2026-01-02 06:00）";
        assert_eq!(
            report.text(),
            format!(
                "Meteosense | Sample Location\n\n\
                 2026-01-01 18:00\n警戒レベル（気圧変化）: 警戒\n現在（0h）の推定海面更正気圧: 1013.2 hPa\n\n\
                 過去の気圧変化（モデル値）\n差分 = 現在の気圧 − 各時点の気圧\n{past}\n\n\
                 今後の気圧変化（予報）\n差分 = 各時点の気圧 − 現在の気圧\n{future}\n\n\
                 今後24時間の最大6時間低下量\n{drop}\n\n今後24時間の最大6時間上昇量\n{rise}\n\
                 大きな気圧上昇を予測\n\n気象データ: Open-Meteo"
            )
        );
        assert_eq!(
            report.complete_upload_body("C0123456789", "F012ABC3456"),
            json!({
                "channel_id": "C0123456789",
                "files": [{"id": "F012ABC3456", "title": "前後24時間の気圧"}],
                "blocks": [
                    {
                        "type": "header",
                        "text": {"type": "plain_text", "text": "Meteosense | Sample Location", "emoji": true}
                    },
                    {
                        "type": "section",
                        "fields": [
                            {"type": "mrkdwn", "text": "*基準時刻*\n2026-01-01 18:00"},
                            {"type": "mrkdwn", "text": "*警戒レベル*\n*警戒*"},
                            {"type": "mrkdwn", "text": "*現在の気圧（0h）*\n`1013.2` hPa"},
                            {"type": "mrkdwn", "text": "*最大6時間低下*\n-6.0 hPa（2026-01-01 18:00 〜 2026-01-02 00:00）"}
                        ]
                    },
                    {"type": "divider"},
                    {
                        "type": "section",
                        "text": {
                            "type": "mrkdwn",
                            "text": format!(
                                "*過去の気圧変化（モデル値）*\n差分 = 現在の気圧 − 各時点の気圧\n```{past}```"
                            )
                        }
                    },
                    {
                        "type": "section",
                        "text": {
                            "type": "mrkdwn",
                            "text": format!(
                                "*今後の気圧変化（予報）*\n差分 = 各時点の気圧 − 現在の気圧\n```{future}```"
                            )
                        }
                    },
                    {"type": "divider"},
                    {
                        "type": "section",
                        "text": {
                            "type": "mrkdwn",
                            "text": format!(
                                "*今後24時間の最大6時間変化*\n低下: {drop}\n上昇: {rise}\n*大きな気圧上昇を予測*"
                            )
                        }
                    },
                    {
                        "type": "context",
                        "elements": [{"type": "mrkdwn", "text": "気象データ: <https://open-meteo.com/|Open-Meteo>"}]
                    }
                ]
            })
        );
    }

    #[test]
    fn accepts_slack_hosted_upload_urls_only() {
        let url = validated_upload_url("https://files.slack.com/upload/v1/ABC123").unwrap();
        assert_eq!(url.as_str(), "https://files.slack.com/upload/v1/ABC123");
        assert_eq!(
            validated_upload_url("https://evil.example/upload")
                .unwrap_err()
                .to_string(),
            "Slackから想定外のアップロード先を受け取りました"
        );
    }

    #[test]
    fn keeps_api_error_codes_safe_to_display() {
        assert_eq!(
            ensure_ok(false, Some("invalid_auth"))
                .unwrap_err()
                .to_string(),
            "Slack APIがエラーを返しました（invalid_auth）"
        );
        assert_eq!(
            ensure_ok(false, Some("not ok <script>"))
                .unwrap_err()
                .to_string(),
            "Slack APIがエラーを返しました"
        );
    }
}
