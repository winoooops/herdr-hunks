# herdr-hunks

[English](README.md) · [简体中文](README.zh-CN.md) · **日本語**

[Herdr](https://herdr.dev) 用の読み取り専用 git ハンクビューアです。
変更ファイル一覧、統合/左右分割の差分表示、クリックできるナビゲーションツールバー、
自動更新を備えています。フェーズ1では、ステージ、ステージ解除、変更の破棄、コメント、
エージェントへの依頼はできません。

## インストール

**GitHub リポジトリとリリースはまだ存在しません。** このチェックアウトを使い、
下記のローカル開発手順で実行してください。リポジトリと最初のリリースの公開後は、
次のコマンドでインストールする予定です。

```sh
herdr plugin install winoooops/herdr-hunks
```

フォーク版のユーザーは、自分の `vimeflow` バイナリで同じコマンドを実行してください。
プラグイン登録先は別々です。インストール時は SHA256 検証済みの macOS または Linux 向け
バイナリを取得します。x86_64 と arm64 に対応します。アセットがない場合、ダウンロードに
失敗した場合、チェックサムが一致しない場合は `cargo build --release` に切り替わり、
Rust 1.88 以降が必要です。

## コマンド

ホスト経由でアクションを呼び出すと、プラグイン ID とペインのコンテキストが渡されます。

```sh
herdr plugin action invoke open --plugin winoooops.hunks
herdr plugin action invoke open-split --plugin winoooops.hunks
herdr plugin action invoke update --plugin winoooops.hunks
```

| アクション | 動作 |
| --- | --- |
| `open` | 呼び出し元ペインの作業ディレクトリをオーバーレイで開き、フォーカスします。閉じると前の表示に戻ります。 |
| `open-split` | 右側の分割ペインで開きます。セッション、呼び出し元、リポジトリ、タイトル、cwd が一致する既存ビューアがあれば、そこにフォーカスします。 |
| `update` | GitHub からのインストールでは最新の安定版タグをインストールします。ローカルリンクや不明な取得元は拒否します。GitHub リポジトリとリリースはまだ存在しないため、公開後に使用してください。 |

単独実行は `herdr-hunks [PATH]` または `herdr-hunks tui [PATH]` です。
PATH の既定値はカレントディレクトリです。標準入力と標準出力の両方が端末である必要があります。
`--version` でバージョンを表示します。アクションの診断情報は
`herdr plugin log list --plugin winoooops.hunks --limit 1` で確認できます。

## キーバインド設定

ホストの設定に以下を追加します。

```toml
[[keys.command]]
key = "prefix+d"
type = "plugin_action"
command = "winoooops.hunks.open"
description = "Open the hunk viewer"
```

分割ペインのアクションを割り当てる場合は、`command` を
`"winoooops.hunks.open-split"` に変更します。キーバインドの自動登録は行いません。

## キー操作

| キー | 操作 |
| --- | --- |
| `j` / `k`、Down / Up | 次の行 / 前の行 |
| Ctrl+D / Ctrl+U、PageDown / PageUp | 半ページ下 / 上 |
| `[` / `]` | 前のハンク / 次のハンク |
| `n` / `p` | 次のファイル / 前のファイル |
| `h` / `l`、Left / Right | 削除側 / 追加側 |
| `t` | 統合 / 左右分割を切り替え。左右分割には100列以上が必要 |
| `e` / `E` | ファイルパネルの表示切り替え / 固定 |
| `r` | 更新 |
| `g` / `G`、Home / End | 最初の行 / 最後の行 |
| `H` / `L` | 左 / 右へ表示セル8個分スクロール |
| `m` | マウスキャプチャの切り替え |
| `?` | キー一覧を開く。`j` / `k` または Down / Up でスクロール |
| `q` | 終了。キー一覧が開いている場合は先に一覧を閉じる |
| Esc | キー一覧を閉じる |
| Ctrl+C | キー一覧の表示中も含め、直ちに終了 |

矢印、PageDown、PageUp、Home、End の別名キーは修飾キーなしでのみ動作します。
フェーズ1では `s d D i I u U x v y Y @ c /` を予約し、操作を割り当てていません。

## マウス

ツールバーの操作項目、ファイル行、差分行をクリックできます。ホイールは3行ずつスクロールします。
`m` でキャプチャを無効にすると、端末本来のテキスト選択が使えます。再度押すと有効になります。
ヘルプの表示中はホイールでヘルプをスクロールします。

## 設定

`config.toml` の配置先は `$HERDR_PLUGIN_CONFIG_DIR`、それがなければ
`${XDG_CONFIG_HOME:-$HOME/.config}/herdr-hunks` です。

```toml
[view]
mode = "auto"     # auto, unified, split
files = "auto"    # auto, pinned, hidden

[input]
mouse = true
```

初期幅に応じて、auto 表示モードは120列以上で左右分割を選び、auto ファイルパネルは100列以上で固定されます。
左右分割を選んでいても100列未満では一時的に統合表示になり、幅を広げると元に戻ります。
40×10 未満ではサイズに関する案内だけを表示します。
無効な設定は項目ごとに既定値へ戻して通知します。診断情報は `$HERDR_PLUGIN_STATE_DIR` 内の
`config-problems.log`、それがなければ `${XDG_STATE_HOME:-$HOME/.local/state}/herdr-hunks` に書き込みます。

ディレクトリ設定には絶対パスが必要です。空の値や相対パスは無視し、次の絶対パス設定を試します。
有効な状態ディレクトリがなくても分割ペインは開きますが、既存ペインは再利用せず、理由を表示します。
リポジトリの場所から状態ディレクトリを決めることはありません。

## 動作要件

- macOS または Linux。標準入力と標準出力が端末に接続されていること。
- PATH 上の Git 2.31 以降。
- プラグインのアクションには Herdr 0.8.0 以降が必要。単独実行ではホスト不要。
- ローカルビルドやインストール時の代替ビルドには Rust 1.88 以降が必要。

## 既知の制限

このフェーズは読み取り専用で、ステージ操作とコメントは未実装です。
[PORT-SURFACE.md に K1–K6 を記載しています](PORT-SURFACE.md#known-defects)。
K1–K4 は凍結コードの変更操作に関する不具合で、フェーズ1では到達しません。
K5 により、変更または追加をステージした後にファイルを削除すると、ステージ済みの部分が隠れる場合があります。
K6 では、新しい要求に置き換わった遅い差分要求の git プロセスが蓄積することがあります。
差分表示は最大 200,000 行まで保持し、超過時は切り詰めを示す行を表示します。
`GIT_NO_LAZY_FETCH` は Git 2.45 以降で有効です。古い Git は部分クローンの読み取り時に不足オブジェクトを取得する場合があります。
シンタックスハイライトとコンテキストの展開は未対応です。

## ロードマップ

P1：読み取り専用の閲覧。P2：ステージ、ステージ解除、変更の破棄と不具合修正。
P3：コメントとエージェントへの依頼。P4：返信とスレッド。P5：レビューの委託。
[設計仕様](docs/superpowers/specs/2026-09-18-hunks-roadmap-p1-viewer-design.md)を参照してください。

## ローカル開発

このチェックアウトで実行します。

```sh
cargo build --release
herdr plugin link "$PWD"
herdr plugin action invoke open --plugin winoooops.hunks
```

`plugin link` は `[[build]]` を実行しないため、先にビルドしてください。
フォーク版では自分の `vimeflow` バイナリに置き換えます。
`cargo build --no-default-features` で、ビューアを含まない単独アクション用のビルドができます。

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -- --test-threads=1
cargo check --no-default-features
scripts/port-check.sh /path/to/vimeflow
sh scripts/port-check-selftest.sh /path/to/vimeflow
```

テストは HOME 配下にテストデータを作ります。書き込み可能で、git チェックアウトの外にあるディレクトリを使ってください。
参照用チェックアウトにはコミット `91e45b1c` が必要です。これらの検証は参照先を変更しません。
CI は `winoooops/vimeflow` からそのコミットの取得を試みます。非公開リポジトリの場合は、読み取り権限を持つ
`VIMEFLOW_READ_TOKEN` Actions secret を設定してください。取得できない場合は案内を表示し、
両方の移植検証をスキップします。上記のローカル検証コマンドは引き続き利用できます。

今後タグで起動するリリースワークフローは4ターゲットをビルドし、タグと Cargo.toml のバージョンの一致を確認します。
Cargo.toml、Cargo.lock、herdr-plugin.toml のバージョンを揃えてください。
`HERDR_HUNKS_RELEASE_BASE=file:///absolute/fixture` を指定すれば、GitHub に接続せずローカルアセットで配布処理を検証できます。

## ライセンス

[Apache-2.0](LICENSE)。vimeflow と herdr-agent-watcher から移植したコードを含みます。
出典と変更内容は [PORT-SURFACE.md](PORT-SURFACE.md) を参照してください。
