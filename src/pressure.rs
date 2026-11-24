use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, TimeDelta, Utc};

const OFFSETS_HOURS: [usize; 5] = [1, 3, 6, 12, 24];
pub(crate) const HORIZON_HOURS: usize = 24;
const WINDOW_HOURS: usize = 6;
const TENTHS_PER_HPA: f64 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pressure(f64);

impl Pressure {
    pub(crate) fn from_hpa(value: f64) -> Result<Self> {
        let tenths = (value * TENTHS_PER_HPA).round();
        ensure!(
            (1.0..=f64::from(i32::MAX)).contains(&tenths),
            "気圧に不正な値が含まれています"
        );
        Ok(Self(value))
    }

    pub(crate) fn hpa(self) -> f64 {
        self.0
    }

    pub(crate) fn rounded_hpa(self) -> f64 {
        f64::from(self.tenths()) / TENTHS_PER_HPA
    }

    fn tenths(self) -> i32 {
        (self.0 * TENTHS_PER_HPA).round() as i32
    }

    fn change_since(self, earlier: Self) -> PressureChange {
        PressureChange(self.tenths() - earlier.tenths())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PressureChange(i32);

impl PressureChange {
    pub(crate) fn hpa(self) -> f64 {
        f64::from(self.0) / TENTHS_PER_HPA
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlertLevel {
    Normal,
    SlightCaution,
    Caution,
    Warning,
}

impl AlertLevel {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Normal => "通常",
            Self::SlightCaution => "やや注意",
            Self::Caution => "注意",
            Self::Warning => "警戒",
        }
    }
}

const DROP_THRESHOLDS: [(PressureChange, AlertLevel); 3] = [
    (PressureChange(-60), AlertLevel::Warning),
    (PressureChange(-40), AlertLevel::Caution),
    (PressureChange(-20), AlertLevel::SlightCaution),
];
const LARGE_RISE_THRESHOLD: PressureChange = PressureChange(60);

fn alert_for(drop: PressureChange) -> AlertLevel {
    DROP_THRESHOLDS
        .iter()
        .find(|(threshold, _)| drop <= *threshold)
        .map_or(AlertLevel::Normal, |(_, level)| *level)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WindowChange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub change: PressureChange,
}

impl WindowChange {
    pub(crate) fn is_large_rise(&self) -> bool {
        self.change >= LARGE_RISE_THRESHOLD
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct IntervalChange {
    pub offset_hours: i64,
    pub pressure: Pressure,
    pub change: PressureChange,
}

#[derive(Debug)]
pub(crate) struct Analysis {
    pub reference: DateTime<Utc>,
    pub current: Pressure,
    pub past: Vec<IntervalChange>,
    pub future: Vec<IntervalChange>,
    pub max_drop: Option<WindowChange>,
    pub max_rise: Option<WindowChange>,
    pub alert: AlertLevel,
}

pub(crate) type HourlyPressure = BTreeMap<DateTime<Utc>, Pressure>;

pub(crate) fn analyze(hourly: &HourlyPressure, reference: DateTime<Utc>) -> Result<Analysis> {
    let pressure_at = |time| {
        hourly
            .get(&time)
            .copied()
            .with_context(|| format!("必要な気圧データがありません（{time}）"))
    };

    let forecast = (0..=HORIZON_HOURS)
        .map(|hours| pressure_at(reference + TimeDelta::hours(hours as i64)))
        .collect::<Result<Vec<_>>>()?;
    let current = forecast[0];

    let past = OFFSETS_HOURS
        .into_iter()
        .rev()
        .map(|hours| {
            let offset_hours = -(hours as i64);
            let earlier = pressure_at(reference + TimeDelta::hours(offset_hours))?;
            Ok(IntervalChange {
                offset_hours,
                pressure: earlier,
                change: current.change_since(earlier),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let future = OFFSETS_HOURS
        .into_iter()
        .map(|hours| IntervalChange {
            offset_hours: hours as i64,
            pressure: forecast[hours],
            change: forecast[hours].change_since(current),
        })
        .collect();

    let mut max_drop: Option<WindowChange> = None;
    let mut max_rise: Option<WindowChange> = None;
    for (offset, window) in forecast.windows(WINDOW_HOURS + 1).enumerate() {
        let start = reference + TimeDelta::hours(offset as i64);
        let change = window[WINDOW_HOURS].change_since(window[0]);
        let window_change = WindowChange {
            start,
            end: start + TimeDelta::hours(WINDOW_HOURS as i64),
            change,
        };
        if change < PressureChange(0) && max_drop.as_ref().is_none_or(|drop| change < drop.change) {
            max_drop = Some(window_change);
        } else if change > PressureChange(0)
            && max_rise.as_ref().is_none_or(|rise| change > rise.change)
        {
            max_rise = Some(window_change);
        }
    }

    let alert = max_drop
        .as_ref()
        .map_or(AlertLevel::Normal, |drop| alert_for(drop.change));
    Ok(Analysis {
        reference,
        current,
        past,
        future,
        max_drop,
        max_rise,
        alert,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn reference() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 18, 0, 0).unwrap()
    }

    fn hourly_series(pressure: impl Fn(i64) -> i32) -> HourlyPressure {
        (-24..=24)
            .map(|offset| {
                (
                    reference() + TimeDelta::hours(offset),
                    Pressure(f64::from(pressure(offset)) / TENTHS_PER_HPA),
                )
            })
            .collect()
    }

    #[test]
    fn calculates_later_minus_earlier_for_each_interval() {
        let hourly = hourly_series(|offset| 10130 + offset as i32 * 2);
        let result = analyze(&hourly, reference()).unwrap();
        let point = |offset_hours, pressure, change| IntervalChange {
            offset_hours,
            pressure: Pressure(f64::from(pressure) / TENTHS_PER_HPA),
            change: PressureChange(change),
        };
        assert_eq!(result.current, Pressure(1013.0));
        assert_eq!(
            result.past,
            [
                point(-24, 10082, 48),
                point(-12, 10106, 24),
                point(-6, 10118, 12),
                point(-3, 10124, 6),
                point(-1, 10128, 2),
            ]
        );
        assert_eq!(
            result.future,
            [
                point(1, 10132, 2),
                point(3, 10136, 6),
                point(6, 10142, 12),
                point(12, 10154, 24),
                point(24, 10178, 48),
            ]
        );
        assert_eq!(result.max_drop, None);
        assert_eq!(result.alert, AlertLevel::Normal);
    }

    #[test]
    fn finds_extrema_across_midnight_and_keeps_earliest_tie() {
        let hourly = hourly_series(|offset| match offset {
            1..=6 => 10130 - offset as i32 * 8,
            7..=12 => 10082 + (offset as i32 - 6) * 10,
            13..=24 => 10142 + (offset as i32 - 12) * 10,
            _ => 10130,
        });
        let result = analyze(&hourly, reference()).unwrap();
        assert_eq!(
            result.max_drop,
            Some(WindowChange {
                start: reference(),
                end: Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
                change: PressureChange(-48),
            })
        );
        let rise = result.max_rise.unwrap();
        assert_eq!(
            rise,
            WindowChange {
                start: Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
                end: Utc.with_ymd_and_hms(2026, 1, 2, 6, 0, 0).unwrap(),
                change: PressureChange(60),
            }
        );
        assert_eq!(result.alert, AlertLevel::Caution);
    }

    #[test]
    fn reports_missing_forecast_hour_instead_of_classifying_a_partial_forecast() {
        let mut hourly = hourly_series(|_| 10130);
        hourly.remove(&(reference() + TimeDelta::hours(2)));
        assert_eq!(
            analyze(&hourly, reference()).unwrap_err().to_string(),
            "必要な気圧データがありません（2026-01-01 20:00:00 UTC）"
        );
    }

    #[test]
    fn applies_inclusive_alert_thresholds() {
        assert_eq!(alert_for(PressureChange(-19)), AlertLevel::Normal);
        assert_eq!(alert_for(PressureChange(-20)), AlertLevel::SlightCaution);
        assert_eq!(alert_for(PressureChange(-39)), AlertLevel::SlightCaution);
        assert_eq!(alert_for(PressureChange(-40)), AlertLevel::Caution);
        assert_eq!(alert_for(PressureChange(-59)), AlertLevel::Caution);
        assert_eq!(alert_for(PressureChange(-60)), AlertLevel::Warning);
    }

    #[test]
    fn rounds_each_pressure_to_tenths_before_calculating_a_change() {
        let current = Pressure::from_hpa(1013.24).unwrap();
        let earlier = Pressure::from_hpa(1011.25).unwrap();

        assert_eq!(current.change_since(earlier), PressureChange(19));
    }
}
