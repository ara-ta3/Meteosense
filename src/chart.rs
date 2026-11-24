use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use plotters::{prelude::*, series::DashedLineSeries};
use tempfile::NamedTempFile;

use crate::pressure::{HORIZON_HOURS, HourlyPressure};

const PNG_SIZE: (u32, u32) = (1200, 680);
const FONT_FAMILY: &str = if cfg!(target_os = "macos") {
    "Hiragino Sans"
} else {
    "Noto Sans CJK JP"
};

pub(crate) fn render_png(
    hourly: &HourlyPressure,
    reference: DateTime<Utc>,
    timezone: Tz,
) -> Result<Vec<u8>> {
    let points = chart_points(hourly, reference)?;
    let png =
        NamedTempFile::with_suffix(".png").context("グラフ用の一時ファイルを作成できません")?;
    draw_png(&points, reference.with_timezone(&timezone), png.path())
        .context("気圧グラフを描画できません")?;
    fs::read(png.path()).context("描画した気圧グラフを読み取れません")
}

pub(crate) fn save_png(png: &[u8], output: &Path) -> Result<()> {
    let directory = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut file = NamedTempFile::with_suffix_in(".png", directory)
        .context("グラフ出力先に一時ファイルを作成できません")?;
    file.write_all(png)
        .and_then(|()| file.flush())
        .context("気圧グラフを一時ファイルへ書き込めません")?;
    file.persist_noclobber(output)
        .map_err(|error| error.error)
        .with_context(|| format!("気圧グラフを保存できません（{}）", output.display()))?;
    Ok(())
}

fn chart_points(hourly: &HourlyPressure, reference: DateTime<Utc>) -> Result<Vec<(i32, f64)>> {
    let horizon = HORIZON_HOURS as i32;
    (-horizon..=horizon)
        .map(|offset| {
            let time = reference + TimeDelta::hours(i64::from(offset));
            let pressure = hourly
                .get(&time)
                .with_context(|| format!("グラフに必要な気圧データがありません（{time}）"))?;
            Ok((offset, pressure.hpa()))
        })
        .collect()
}

fn draw_png(points: &[(i32, f64)], reference: DateTime<Tz>, output: &Path) -> Result<()> {
    let horizon = HORIZON_HOURS as i32;
    let (min, max) = points
        .iter()
        .map(|(_, hpa)| *hpa)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), hpa| {
            (min.min(hpa), max.max(hpa))
        });
    let padding = ((max - min) * 0.08).max(0.5);
    let pressure_range = (min - padding)..(max + padding);
    let current = points[HORIZON_HOURS];

    let ink = RGBColor(20, 36, 56);
    let muted = RGBColor(70, 90, 112);
    let past_color = RGBColor(25, 91, 116);
    let future_color = RGBColor(33, 91, 176);
    let text = |size, color: &RGBColor| (FONT_FAMILY, size).into_font().color(color);
    let root = BitMapBackend::new(output, PNG_SIZE).into_drawing_area();
    root.fill(&WHITE)?;
    root.draw(&Text::new("前後24時間の気圧", (88, 27), text(39, &ink)))?;
    root.draw(&Text::new(
        format!(
            "{} 基準・現在 {:.1} hPa・1時間ごと",
            reference.format("%Y-%m-%d %H:%M %Z %:z"),
            current.1,
        ),
        (89, 80),
        text(21, &muted),
    ))?;
    root.draw(&PathElement::new(
        [(681, 126), (722, 126)],
        past_color.stroke_width(4),
    ))?;
    root.draw(&Text::new("過去（モデル値）", (731, 113), text(19, &muted)))?;
    for x in [912, 928, 944] {
        root.draw(&PathElement::new(
            [(x, 126), (x + 9, 126)],
            future_color.stroke_width(4),
        ))?;
    }
    root.draw(&Text::new("今後（予報）", (963, 113), text(19, &muted)))?;
    root.draw(&Text::new(
        "気象データ  Open-Meteo",
        (89, 638),
        text(18, &muted),
    ))?;

    let body = root.margin(146, 58, 10, 42);
    let mut chart = ChartBuilder::on(&body)
        .x_label_area_size(66)
        .y_label_area_size(78)
        .build_cartesian_2d(
            (-horizon - 1..horizon + 1).with_key_points((-horizon..=horizon).step_by(6).collect()),
            pressure_range.clone(),
        )?;
    chart.draw_series(std::iter::once(Rectangle::new(
        [(0, pressure_range.start), (horizon + 1, pressure_range.end)],
        RGBColor(240, 246, 252).filled(),
    )))?;
    chart
        .configure_mesh()
        .disable_x_mesh()
        .y_labels(7)
        .x_label_formatter(&|hour| match *hour {
            0 => "0h".into(),
            hour => format!("{hour:+}h"),
        })
        .axis_style(RGBColor(172, 188, 205))
        .bold_line_style(RGBColor(220, 229, 238))
        .light_line_style(TRANSPARENT)
        .label_style(text(17, &muted))
        .axis_desc_style(text(18, &muted))
        .x_desc("基準時刻からの時間")
        .y_desc("海面更正気圧（hPa）")
        .draw()?;
    chart.draw_series(std::iter::once(PathElement::new(
        [(0, pressure_range.start), (0, pressure_range.end)],
        RGBColor(129, 148, 167).stroke_width(2),
    )))?;
    chart.draw_series(LineSeries::new(
        points[..=HORIZON_HOURS].iter().copied(),
        past_color.stroke_width(4),
    ))?;
    chart.draw_series(DashedLineSeries::new(
        points[HORIZON_HOURS..].iter().copied(),
        11,
        7,
        future_color.stroke_width(4),
    ))?;
    chart.draw_series(
        points[..HORIZON_HOURS]
            .iter()
            .map(|point| Circle::new(*point, 4, past_color.filled())),
    )?;
    chart.draw_series(
        points[HORIZON_HOURS + 1..]
            .iter()
            .map(|point| Circle::new(*point, 3, future_color.filled())),
    )?;
    chart.draw_series(std::iter::once(Circle::new(current, 7, ink.filled())))?;
    root.present()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pressure::Pressure;
    use chrono::TimeZone;

    fn reference() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap()
    }

    fn hourly(value: f64) -> HourlyPressure {
        (-24..=24)
            .map(|offset| {
                (
                    reference() + TimeDelta::hours(offset),
                    Pressure::from_hpa(value).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn uses_every_hour_and_preserves_source_pressure() {
        let mut data = hourly(1013.25);
        let before_reference = reference() - TimeDelta::hours(2);
        data.insert(before_reference, Pressure::from_hpa(1011.99).unwrap());

        let points = chart_points(&data, reference()).unwrap();

        assert_eq!(points.len(), 49);
        assert_eq!(points[0], (-24, 1013.25));
        assert_eq!(points[22], (-2, 1011.99));
        assert_eq!(points[24], (0, 1013.25));
        assert_eq!(points[48], (24, 1013.25));
    }

    #[test]
    fn reports_the_timestamp_of_a_missing_hour() {
        let mut data = hourly(1013.2);
        data.remove(&(reference() - TimeDelta::hours(2)));

        let error = chart_points(&data, reference()).unwrap_err();

        assert_eq!(
            error.to_string(),
            "グラフに必要な気圧データがありません（2026-01-01 10:00:00 UTC）"
        );
    }

    #[test]
    fn writes_a_png_even_when_every_pressure_is_the_same() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("pressure.png");
        let png = render_png(&hourly(1013.2), reference(), chrono_tz::Asia::Tokyo).unwrap();
        save_png(&png, &output).unwrap();

        let saved = std::fs::read(output).unwrap();
        assert_eq!(&saved[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(saved, png);
        assert_eq!(u32::from_be_bytes(saved[16..20].try_into().unwrap()), 1200);
        assert_eq!(u32::from_be_bytes(saved[20..24].try_into().unwrap()), 680);
    }

    #[test]
    fn preserves_an_existing_output_file() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("pressure.png");
        std::fs::write(&output, b"existing contents").unwrap();
        let png = render_png(&hourly(1013.2), reference(), chrono_tz::Asia::Tokyo).unwrap();

        let error = save_png(&png, &output).unwrap_err();

        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::AlreadyExists)
        );
        assert_eq!(std::fs::read(output).unwrap(), b"existing contents");
    }
}
