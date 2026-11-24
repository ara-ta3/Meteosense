mod chart;
mod config;
mod http;
mod pressure;
mod slack;
mod weather;

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use chrono::{Timelike, Utc};
use clap::Parser;

use crate::config::{Config, WebhookUrl};

#[derive(Parser)]
#[command(about = "毎時の気圧データを分析してSlackに通知します")]
struct Cli {
    #[arg(long, help = "Slackに投稿せず、分析結果を標準出力に表示する")]
    dry_run: bool,
    #[arg(
        long,
        value_name = "PNG",
        help = "指定したパスに気圧グラフのPNGを書き出す"
    )]
    chart: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("meteosense: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let config = Config::from_env()?;
    let webhook = if cli.dry_run {
        None
    } else {
        Some(WebhookUrl::from_env()?)
    };

    let now = Utc::now();
    let reference = now
        .date_naive()
        .and_hms_opt(now.hour(), 0, 0)
        .context("現在時刻を毎時の基準時刻に変換できません")?
        .and_utc();
    let agent = http::agent();
    let hourly = weather::fetch(&agent, &config, reference)?;
    let analysis = pressure::analyze(&hourly, reference)?;
    if let Some(path) = cli.chart {
        chart::write_png(&hourly, reference, config.timezone, &path)?;
    }
    let report = slack::Report::new(&analysis, &config.location_name, config.timezone);

    match webhook {
        Some(webhook) => slack::post(&agent, &webhook, &report),
        None => writeln!(io::stdout().lock(), "{}", report.text())
            .context("分析結果を標準出力に書き込めません"),
    }
}
