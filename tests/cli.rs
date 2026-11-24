use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_meteosense"))
        .env_clear()
        .env("METEOSENSE_LATITUDE", "0")
        .env("METEOSENSE_LONGITUDE", "0")
        .env("METEOSENSE_LOCATION_NAME", "Sample Location")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_is_available_without_a_webhook() {
    let output = run(&["--help"]);
    assert_eq!(output.status.code(), Some(0));
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--dry-run"), "{help}");
    assert!(help.contains("--chart <PNG>"), "{help}");
    assert!(help.contains("--help"), "{help}");
    assert_eq!(output.stderr, b"");
}

#[test]
fn posting_mode_requires_a_webhook_before_requesting_weather() {
    let output = run(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "meteosense: SLACK_WEBHOOK_URL を設定してください\n"
    );
}
