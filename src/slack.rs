use std::fmt::Write as _;

use anyhow::{Result, ensure};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde_json::{Value, json};
use ureq::Agent;

use crate::config::WebhookUrl;
use crate::http::{request_error, response_error};
use crate::pressure::{Analysis, IntervalChange, WindowChange};

const SERVICE: &str = "Slack";
const ATTRIBUTION: &str = "気象データ: Open-Meteo";
const ATTRIBUTION_LINK: &str = "気象データ: <https://open-meteo.com/|Open-Meteo>";

pub(crate) struct Report {
    title: String,
    sections: [String; 4],
}

impl Report {
    pub(crate) fn new(analysis: &Analysis, location_name: &str, timezone: Tz) -> Self {
        let summary = format!(
            "{}\n警戒レベル（気圧変化）: {}\n現在の推定海面更正気圧: {:.1} hPa",
            local_time(analysis.reference, timezone),
            analysis.alert.label(),
            analysis.current.hpa()
        );
        let windows = format!(
            "今後24時間の最大6時間低下量\n{}\n\n今後24時間の最大6時間上昇量\n{}{}",
            window_text(analysis.max_drop.as_ref(), timezone),
            window_text(analysis.max_rise.as_ref(), timezone),
            if analysis
                .max_rise
                .as_ref()
                .is_some_and(WindowChange::is_large_rise)
            {
                "\n大きな気圧上昇を予測"
            } else {
                ""
            }
        );
        Self {
            title: format!("Meteosense | {location_name}"),
            sections: [
                summary,
                interval_text("過去の気圧変化（モデル値）", &analysis.past),
                interval_text("今後の気圧変化（予報）", &analysis.future),
                windows,
            ],
        }
    }

    pub(crate) fn text(&self) -> String {
        format!(
            "{}\n\n{}\n\n{ATTRIBUTION}",
            self.title,
            self.sections.join("\n\n")
        )
    }

    fn payload(&self) -> Value {
        let mut blocks = vec![json!({
            "type": "header",
            "text": {"type": "plain_text", "text": self.title}
        })];
        blocks.extend(self.sections.iter().map(|text| {
            json!({
                "type": "section",
                "text": {"type": "plain_text", "text": text}
            })
        }));
        blocks.push(json!({
            "type": "context",
            "elements": [{"type": "mrkdwn", "text": ATTRIBUTION_LINK}]
        }));
        json!({
            "text": self.text(),
            "mrkdwn": false,
            "blocks": blocks
        })
    }
}

pub(crate) fn post(agent: &Agent, webhook: &WebhookUrl, report: &Report) -> Result<()> {
    let mut response = agent
        .post(webhook.as_str())
        .send_json(report.payload())
        .map_err(|error| request_error(SERVICE, error))?;
    if !response.status().is_success() {
        return Err(response_error(SERVICE, response.status().as_u16()));
    }
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| request_error(SERVICE, error))?;
    ensure!(
        body.trim() == "ok",
        "Slackから投稿成功の応答を受け取れませんでした"
    );
    Ok(())
}

fn interval_text(title: &str, intervals: &[IntervalChange]) -> String {
    let mut text = title.to_owned();
    for interval in intervals {
        write!(
            text,
            "\n{:>2}h: {:+.1} hPa",
            interval.hours,
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
        .format("%Y-%m-%d %H:%M %Z")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pressure::{HourlyPressure, Pressure, analyze};
    use chrono::{TimeDelta, TimeZone};

    #[test]
    fn generates_block_kit_payload_with_local_times_and_accessible_fallback() {
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
        let report = Report::new(&analysis, "Sample Location", chrono_tz::Asia::Tokyo);

        let summary = "2026-01-01 18:00 JST\n警戒レベル（気圧変化）: 警戒\n現在の推定海面更正気圧: 1013.2 hPa";
        let past = "過去の気圧変化（モデル値）\n 1h: -1.0 hPa\n 3h: -3.0 hPa\n 6h: -6.0 hPa\n12h: -12.0 hPa\n24h: -24.0 hPa";
        let future = "今後の気圧変化（予報）\n 1h: -1.0 hPa\n 3h: -3.0 hPa\n 6h: -6.0 hPa\n12h: +0.0 hPa\n24h: +12.0 hPa";
        let windows = "今後24時間の最大6時間低下量\n-6.0 hPa（2026-01-01 18:00 JST 〜 2026-01-02 00:00 JST）\n\n今後24時間の最大6時間上昇量\n+6.0 hPa（2026-01-02 00:00 JST 〜 2026-01-02 06:00 JST）\n大きな気圧上昇を予測";
        assert_eq!(
            report.payload(),
            json!({
                "text": format!("Meteosense | Sample Location\n\n{summary}\n\n{past}\n\n{future}\n\n{windows}\n\n気象データ: Open-Meteo"),
                "mrkdwn": false,
                "blocks": [
                    {"type": "header", "text": {"type": "plain_text", "text": "Meteosense | Sample Location"}},
                    {"type": "section", "text": {"type": "plain_text", "text": summary}},
                    {"type": "section", "text": {"type": "plain_text", "text": past}},
                    {"type": "section", "text": {"type": "plain_text", "text": future}},
                    {"type": "section", "text": {"type": "plain_text", "text": windows}},
                    {"type": "context", "elements": [{"type": "mrkdwn", "text": "気象データ: <https://open-meteo.com/|Open-Meteo>"}]}
                ]
            })
        );
    }
}
