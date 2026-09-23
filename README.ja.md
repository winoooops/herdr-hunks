# herdr-hunks

[English](README.md) · [简体中文](README.zh-CN.md) · **日本語**

[Herdr](https://herdr.dev) 用の読み取り専用 git ハンクビューアです。
変更ファイル一覧、統合/左右分割の差分表示、クリックできるナビゲーションツールバー、
自動更新を備えています。フェーズ1では、ステージ、ステージ解除、変更の破棄、コメント、
エージェントへの依頼はできません。

## インストール

[GitHub](https://github.com/winoooops/herdr-hunks) からインストールします。

```sh
herdr plugin install winoooops/herdr-hunks
```

フォーク版のユーザーは、自分の `vimeflow` バイナリで同じコマンドを実行してください。
プラグイン登録先は別々です。インストール時は、チェックアウトしたバージョンに対応する
リリースから SHA256 検証済みのバイナリを取得します。macOS と Linux の x86_64 および
arm64 に対応します。アセットがない場合、ダウンロードに失敗した場合、チェックサムが
一致しない場合は `cargo build --release` によるソースビルドに切り替わり、Rust 1.88
以降が必要になります。

## コマンド

ホスト経由でアクションを呼び出すと、プラグイン ID とペインのコンテキストが渡されます。

```sh
herdr plugin action invoke open --plugin winoooops.hunks
herdr plugin action invoke open-split --plugin winoooops.hunks
herdr plugin action invoke update --plugin winoooops.hunks
```

| アクション | 動作 |
| --- | --- |
| `open` | 呼び出し元ペインの作業ディレクトリをポップアップダイアログで開き、フォーカスします。既定の幅と高さは80%です。ホストは通常のワークスペース表示である必要があります。 |
| `open-split` | 右側の分割ペインで開きます。セッション、呼び出し元、リポジトリ、タイトル、cwd が一致する既存ビューアがあれば、そこにフォーカスします。 |
| `update` | GitHub からのインストールでは、最新のリリースタグが実行中のバージョンより新しい場合にそれをインストールし、そうでなければ最新である旨を表示します。ローカルリンクや不明な取得元は拒否します。 |

単独実行は `herdr-hunks [PATH]` または `herdr-hunks tui [PATH]` です。
PATH の既定値はカレントディレクトリです。標準入力と標準出力の両方が端末である必要があります。
`--version` は `herdr-hunks 0.0.2` を表示します。アクションの診断情報は
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
| `b` | スコープを切り替え：worktree <-> branch |
| `B` | 比較の基準を選択 |
| `g` / `G`、Home / End | 最初の行 / 最後の行 |
| `H` / `L` | 左 / 右へ表示セル8個分スクロール |
| `m` | マウスキャプチャの切り替え |
| `?` | キー一覧を開く。`j` / `k` または Down / Up でスクロール |
| `q` | 終了。キー一覧が開いている場合は先に一覧を閉じる |
| Esc | キー一覧または基準ピッカーを閉じる。どちらもなければポップアップビューアを閉じる |
| Ctrl+C | キー一覧や基準ピッカーの表示中も含め、直ちに終了 |

矢印、PageDown、PageUp、Home、End の別名キーは修飾キーなしでのみ動作します。
フェーズ1では `s d D i I u U x v y Y @ c /` を予約し、操作を割り当てていません。

## マウス

ツールバーの操作項目は左右に余白のある太字・反転表示のボタンです。
無効なボタンは薄く表示され、クリックできません。ファイルが2個未満ならファイル移動、
読み込み済みハンクが2個未満ならハンク移動、要求モードが統合表示で100列未満なら表示切り替えボタンが無効です。
基準を解決できていない場合は、スコープボタンも無効です。
ホバー中のボタンは明るくなり、フッターに説明とキーが表示されます。ファイル行は太字になります。
ポインターを外すと通常のヒントに戻ります。ボタン、ファイル行、差分行をクリックできます。
ホイールは3行ずつスクロールします。
`m` でキャプチャを無効にすると、端末本来のテキスト選択が使えます。再度押すと有効になります。無効にするとホバー状態も消えます。
ヘルプの表示中はホイールでヘルプをスクロールします。基準ピッカーでは行をクリックして選択し、ホイールでカーソルを移動できます。

## 設定

`config.toml` の配置先は `$HERDR_PLUGIN_CONFIG_DIR`、それがなければ
`${XDG_CONFIG_HOME:-$HOME/.config}/herdr-hunks` です。

```toml
[view]
mode = "auto"      # auto, unified, split
files = "auto"     # auto, pinned, hidden
scope = "worktree" # worktree, branch

[input]
mouse = true

[base]
ref = "main"       # 任意のリビジョン。「ブランチスコープ」を参照

[popup]
width = "80%"
height = "80%"
```

ポップアップのサイズは20以上の整数（枠を含むセル数）、または "20%" から "100%" の割合で指定します。
無効なサイズは項目ごとに "80%" に戻り、`open` が標準エラーに問題を表示します。
ファイルがなければ既定値を使い、読み取り不能や構文エラーの場合は問題を表示して既定値を使います。

初期幅に応じて、auto 表示モードは120列以上で左右分割を選び、auto ファイルパネルは100列以上で固定されます。
左右分割を選んでいても100列未満では一時的に統合表示になり、幅を広げると元に戻ります。
40×10 未満ではサイズに関する案内だけを表示します。
無効な view/input/base 設定は項目ごとに既定値へ戻して通知します。診断情報は `$HERDR_PLUGIN_STATE_DIR` 内の
`config-problems.log`、それがなければ `${XDG_STATE_HOME:-$HOME/.local/state}/herdr-hunks` に書き込みます。

ディレクトリ設定には絶対パスが必要です。空の値や相対パスは無視し、次の絶対パス設定を試します。
有効な状態ディレクトリがなくても分割ペインは開きますが、既存ペインは再利用せず、理由を表示します。
リポジトリの場所から状態ディレクトリを決めることはありません。

## ブランチスコープ

ビューアには2つのスコープがあります。`worktree`（既定）は `git status` と同じく、
`HEAD` に対するステージ済み、未ステージ、未追跡の変更を一覧にします。
`branch` は現在のブランチが基準に対して持つすべての変更を一覧にします。
作業ツリーを `merge-base(HEAD, base)` と比較し、コミット済みかどうかに関係なく
パスごとに1行を表示し、未追跡ファイルも追加します。`b` で切り替えます
（ツールバーボタンは `worktree` または `vs <base>` を表示）。
`prefix+f` を `open-split` に割り当てた場合、`prefix`、`f`、`b` の順に押します。
ブランチ上で削除したパスを未追跡ファイルとして作り直した場合は2行になります。

基準は次の順に解決し、コミットを指す最初の値を使います。

1. このワークツリーで `B` により選んだ基準（状態ディレクトリの `bases.json` に保存）；
2. `config.toml` の `[base] ref`；
3. `refs/heads/main`；
4. リモートの既定ブランチ（`refs/remotes/origin/HEAD`）；
5. `refs/heads/master`。

どれも解決できない場合、`b` は `no base branch: set [base] ref or press B` を表示します。

`B` で「Compare against」を開きます。入力するとローカルブランチ、リモートブランチ、タグを
絞り込めます（新しい順に最大200件。残りは直接入力して指定できます）。
`origin/main` や `HEAD~3` など、任意のリビジョンも入力できます。
`Enter` で選択、`Esc` でキャンセルし、先頭の `default (...)` 行で選択を解除します。
一覧から選んだ参照は完全修飾名で git に渡します。自由入力と `[base] ref` はそのまま渡すため、
同名のタグがある場合は `refs/heads/x` と指定してください。選択はワークツリーごとに保存されます。
状態ディレクトリが使えない場合は、現在のセッションでのみ有効です。

ブランチスコープもビューアの他の部分と同じく読み取り専用で、実行する git コマンドに
`merge-base` と `for-each-ref` が加わります。更新のたびに基準を再確認するため、
基準が移動した場合（`main` へのマージや fetch など）もポーリング間隔内に反映されます。

## 動作要件

- macOS または Linux。標準入力と標準出力が端末に接続されていること。
- PATH 上の Git 2.31 以降。
- プラグインのアクションには Herdr 0.8.0 以降が必要。単独実行ではホスト不要。
- ローカルビルドやインストール時の代替ビルドには Rust 1.88 以降が必要。

## 既知の制限

このフェーズは読み取り専用で、ステージ操作とコメントは未実装です。
[PORT-SURFACE.md に K1–K7 を記載しています](PORT-SURFACE.md#known-defects)。
K1–K4 は凍結コードの変更操作に関する不具合で、フェーズ1では到達しません。
K5 により、変更または追加をステージした後にファイルを削除すると、ステージ済みの部分が隠れる場合があります。
K6 では、新しい要求に置き換わった遅い差分要求の git プロセスが蓄積することがあります。
worktree スコープでは、ステージ済み行のパスがファイルからディレクトリに変わると、
ディレクトリのパッチをその行自身のパッチとして解析します（PORT-SURFACE.md の K7）。branch スコープは影響を受けません。
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

`v*` タグを push するとリリースワークフローが起動し、4ターゲットをビルドします。タグと Cargo.toml のバージョンの一致、および `docs/acceptance-p1.md` が PASS であることが必要です。
Cargo.toml、Cargo.lock、herdr-plugin.toml のバージョンを揃えてください。
`HERDR_HUNKS_RELEASE_BASE=file:///absolute/fixture` を指定すれば、GitHub に接続せずローカルアセットで配布処理を検証できます。

## ライセンス

[Apache-2.0](LICENSE)。vimeflow（MIT。表示は
[third-party/vimeflow-LICENSE](third-party/vimeflow-LICENSE)）と herdr-agent-watcher
（Apache-2.0）から移植したコードを含みます。出典と変更内容は [NOTICE](NOTICE) と
[PORT-SURFACE.md](PORT-SURFACE.md) を参照してください。
