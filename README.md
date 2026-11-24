# Meteosense

気象・環境の変化から、体調への影響を把握するためのツール。

## 背景・目的

気圧や気温、湿度などの気象条件の変化は、頭痛や倦怠感などの体調変化に関係することがある。

気象データを定期的に取得・分析し、体調に影響する可能性のある変化を事前に把握できるようにする。

まずは頭痛ーるのような気圧変化の通知機能を実装し、将来的には気象と体調の関係を多角的に分析できるツールを目指す。

## MVP

Open-Meteoから毎時の海面更正気圧を1回取得し、現在までの変化と今後24時間の予報を分析する。通常実行では、気圧変化の警戒レベルにかかわらず毎回Slackに投稿する。`--dry-run` では同じ分析結果を標準出力に表示する。

## macOSでのセットアップ

[公式の手順](https://rust-lang.org/tools/install/)でRust stableを用意し、リポジトリのルートでビルドする。

```sh
cargo build --locked --release
```

環境変数は実行するプロセスに渡す。`.env.example` を使う場合は、次のように作成して値を編集する。`.env` の値はシェルから読み込む。

```sh
(umask 077 && cp -n .env.example .env)
```

| 環境変数 | 設定 |
| --- | --- |
| `METEOSENSE_LATITUDE` | 緯度。−90から90まで |
| `METEOSENSE_LONGITUDE` | 経度。−180から180まで |
| `METEOSENSE_LOCATION_NAME` | 通知に表示する地点名。130文字以内 |
| `METEOSENSE_TIMEZONE` | 表示用のIANAタイムゾーン。省略時は `Asia/Tokyo` |
| `SLACK_WEBHOOK_URL` | [Slack Incoming Webhook](https://docs.slack.dev/messaging/sending-messages-using-incoming-webhooks/) のURL。通常実行時に必須 |

Webhook URLは秘密情報として扱い、`.env` の読み取り権限を利用者に限定する。

```sh
./target/release/meteosense --help

# .env の変数をサブシェル内でエクスポートし、結果を確認する
(set -a && . ./.env && ./target/release/meteosense --dry-run)

# 同じ設定でSlackに投稿する
(set -a && . ./.env && ./target/release/meteosense)
```

エラーは標準エラー出力に表示し、非ゼロのステータスで終了する。

### 気圧グラフの書き出し

`--chart <出力先.png>` を追加すると、前後24時間の気圧グラフをPNGで保存する。

```sh
(set -a && . ./.env && ./target/release/meteosense --dry-run --chart /tmp/pressure.png)
```

Plottersで、Open-Meteoから取得した差分計算前の毎時気圧49点を描画する。基準時刻を縦線で示し、過去のモデル値を実線、今後の予報を破線で表示する。毎時データが欠けている場合や、出力先のファイルが既に存在する場合はエラーになる。

PNGは指定したパスに保存する。Slackには従来のテキスト通知が届く。日本語の描画にはmacOSでヒラギノ角ゴシック、UbuntuでNoto Sans CJK JPを使用する。Ubuntuでビルド・描画する場合は `fontconfig`、`libfontconfig1-dev`、`fonts-noto-cjk` を用意する。

### 毎時実行

#### GitHub Actions

`Pressure notifications` ワークフローは、デフォルトブランチに反映されると毎時5分（UTC）に起動し、Slackへ通知する。リポジトリの **Settings → Secrets and variables → Actions** で次の値を設定する。

- **Secrets** に `METEOSENSE_LATITUDE`、`METEOSENSE_LONGITUDE`、`METEOSENSE_LOCATION_NAME`、`SLACK_WEBHOOK_URL` を登録する。
- 表示タイムゾーンを変更する場合は **Variables** に `METEOSENSE_TIMEZONE` を登録する。省略時は `Asia/Tokyo` を使う。

Actionsから手動実行すると、サンプル地点の通知プレビューが実行ログに表示される。Slackに投稿するのは定期実行時のみ。定期実行は混雑時に遅延・欠落する場合があり、公開リポジトリで60日間アクティビティがないと自動停止する。[GitHub Actionsのスケジュール仕様](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule)

#### macOSのcron

先に手動実行で設定を確認し、`crontab -e` で次のような行を追加する。設定ファイルとバイナリは実際の絶対パスに置き換える。例では毎時5分に実行する。

```cron
5 * * * * set -a && . "/absolute/path/to/Meteosense/.env" && exec "/absolute/path/to/Meteosense/target/release/meteosense"
```

macOSがスリープ中の間はcronが実行されない。Meteosenseは実行時点のデータを取得して終了する。

## 気圧変化の判定

UTCで現在の時刻を時単位に切り下げ、基準時刻の24時間前から24時間後までを取得する。結果の時刻は設定されたタイムゾーンで表示する。気圧は0.1 hPa単位で扱い、過去・未来それぞれの1、3、6、12、24時間について、後の気圧から前の気圧を引く。

通知では基準時刻を `0h` とし、`-24h` から `+24h` まで時系列順に、各時点の気圧と差分を表示する。過去の差分は現在の気圧から各時点の気圧を引き、今後の差分は各時点の気圧から現在の気圧を引く。

今後24時間の範囲で6時間窓を1時間ずつ動かし、窓の終点と始点の差から最大低下・最大上昇とそれぞれの時間帯を求める。同じ値の窓が複数ある場合は最も早い時間帯を表示する。分析に必要な気圧値が欠けている場合は、該当時刻を表示して終了する。

| 最大6時間低下量 | 警戒レベル |
| --- | --- |
| 6 hPa以上 | 警戒 |
| 4 hPa以上、6 hPa未満 | 注意 |
| 2 hPa以上、4 hPa未満 | やや注意 |
| 2 hPa未満、または低下なし | 通常 |

6時間で6 hPa以上の上昇が予測される場合は、大きな気圧上昇として併記する。警戒レベルは気圧変化に対する独自の暫定基準であり、医学的な根拠が確立した基準や症状の予測としては扱えない。

## 依存関係の更新

[Renovate](https://docs.renovatebot.com/) が週次で依存関係を確認し、Cargo と GitHub Actions をそれぞれまとめた PR を作成する。CI が通った PR は GitHub の auto-merge で取り込む。

初回のみ次を有効にする。

1. [Renovate GitHub App](https://github.com/apps/renovate) をこのリポジトリにインストールする
2. **Settings → General → Pull Requests** で **Allow auto-merge** を有効にする
3. 推奨: `main` に branch protection を置き、CI（`rust`）を必須ステータスにする。必須チェックがあると、auto-merge は CI 成功後に実行される

設定はリポジトリ直下の `renovate.json` を参照する。

## 気象データ

出典は [Open-Meteo Weather Forecast API](https://open-meteo.com/en/docs)。海面更正気圧（`pressure_msl`）は地点の標高での気圧とは異なる。過去時刻の値にも気象モデルに基づく推定値が含まれ、観測地点での実測を示すものではない。通知する変化量は取得値からMeteosenseが算出する。

公開APIの無料枠は非商用利用が対象で、毎分・毎時・日ごとに利用上限がある。データはCC BY 4.0で提供される。[Open-Meteo利用条件](https://open-meteo.com/en/terms)
