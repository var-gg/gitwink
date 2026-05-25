# gitwink

[English](README.md) · [한국어](README.ko.md) · **日本語**

[![Release](https://img.shields.io/github/v/release/var-gg/gitwink)](https://github.com/var-gg/gitwink/releases/latest)
[![Microsoft Store](https://img.shields.io/badge/Microsoft%20Store-Available-0078D4?logo=microsoftstore&logoColor=white)](https://apps.microsoft.com/detail/9P0S21GJD53F)

> AIエージェント時代のための、トレイ常駐・読み取り専用 git glance。

**ステータス:** v0.4 — 実用段階。コールドスタートに優しいトレイアプリ。

![gitwink](docs/images/hero.gif)

gitwink はシステムトレイに常駐する。クリックすると、**すべて**のローカル
リポジトリの最近のコミット活動をひと目で確認できる。git **クライアント
ではない** — commit・push・merge・変更、いずれもできない。設計上、
読み取り専用。

## ダウンロード

**Windows — [Microsoft Store](https://apps.microsoft.com/detail/9P0S21GJD53F):**

[**Microsoft Store で gitwink を入手 →**](https://apps.microsoft.com/detail/9P0S21GJD53F)

Store ビルドは認証時に Microsoft が自動的に署名するため、SmartScreen の
警告は表示されない。アップデートも Store が管理する — このチャンネルでは
gitwink のアプリ内アップデーターは無効化される。

**Windows — [Scoop](https://scoop.sh):**

```sh
scoop bucket add var-gg https://github.com/var-gg/scoop-bucket
scoop install gitwink
```

以降のアップデートは `scoop update gitwink`。Scoop はビルドを展開して
インストールするため、SmartScreen の警告は表示されない。

**または直接ダウンロード:**

[**最新リリースをダウンロード →**](https://github.com/var-gg/gitwink/releases/latest)

- **Windows** — `.exe`（NSIS インストーラ）または `.msi`
- **macOS** — `.dmg`（universal）

直接ダウンロードのビルドは現時点で署名されていないため、初回起動時に
Windows SmartScreen / macOS Gatekeeper が警告を出す — 回避手順はリリース
ノートに記載。自分でビルドしたい場合は [開発](#開発) を参照。

## コード署名

インストール経路ごとに信頼パスが異なる:

- **Microsoft Store** — 認証時に Microsoft が自動再署名。SmartScreen は出ない。
- **Scoop** — 展開方式のインストールなので、SmartScreen は出ない。
- **直接ダウンロード**（`.exe` / `.msi`）— 現時点で未署名。gitwink は
  オープンソース向けの [SignPath Foundation](https://signpath.org/) 無償
  コード署名プログラムに参加している（[コード署名ポリシー](CODE_SIGNING_POLICY.md)
  参照）; 承認され次第、SignPath 証明書がこれらの成果物に署名する。

## なぜ作ったか

以前は VS Code に GitLens を常駐させて生活していた。ブランチグラフ、
ヒートマップ化された blame、lens のアノテーション — それが *私の* git
ワークフローだった。そして 2025 年が来た。Cursor、Claude Code、Codex が
実際の編集をこなすようになり、エディタそのものが任意の存在になった。
それでも私を VS Code に引き戻す唯一の理由が GitLens だった。

それは無駄に思えた — コミット履歴をちょっと覗くためだけに IDE を丸ごと
起動するのが。git コマンドはもうエージェントが打つ。私はたまに、何かが
おかしく見えたときだけ、結果をざっと確認したいだけだ。gitwink は *その*
ループに合わせた、可能な限り最小のツールだ — トレイアイコンが展開して
ひと目を提供し、コミットを AI コンテキストとして渡し、そして退く。

commit なし。push なし。merge なし。git の手術が必要なら、エージェント
に頼む。

## ループ

0.5 秒の確認ループ:

```
エージェントがコミット  →  トレイをクリック  →  インライン展開  →  "Copy as AI context"
                                                   →  Claude/Codex に貼り付け
                                                   →  「エージェントは正しくやったか?」
```

## 機能

- システムトレイアイコン（Windows トレイ / macOS メニューバー）。
  クリックでトグル、右クリックで 位置リセット / 設定ファイルを開く /
  終了。
- グローバルホットキー `Ctrl+Shift+G`（Windows）/ `Cmd+Shift+G`
  （macOS）でどこからでもパネルを呼び出し・解除。`settings.json` の
  `panel_hotkey` を編集すれば変更可能（トレイを右クリック →
  「Open settings file…」）。Tauri のショートカット仕様なら何でも
  指定でき、例えば `"Alt+Space"`、`"Ctrl+Alt+Backquote"` など。
  適用には再起動が必要。
- 初回起動時にデフォルトのユーザーディレクトリを探索（`source`、
  `Documents`、`Projects`、`Code`、`Dev`、`repos`、`Desktop`、Windows
  ではすべての非システムドライブ / macOS では `~/Projects`、`~/Code`、
  `~/Documents`、`~/Developer`）。結果は
  `%APPDATA%\gg.var.gitwink\cache.db` の SQLite にキャッシュ。
- すべてのリポジトリを横断する統合コミットタイムライン。上部のチップ
  でフィルタ: Repo（検索 + ピン留め）、期間（24h / 3d / 7d / 30d /
  All）、作成者（カウント付き複数選択）。
- 行ごとのマーカー — `●` コミット · `◆` マージ · `★` タグ付き。現在
  チェックアウト中のブランチに無いコミットにはブランチラベルバッジ。
- 単一リポジトリモード: リポジトリを 1 つ選ぶとパネルがブランチ別
  ビューに切り替わる。カスタム SVG DAG レーン描画（8 色パレット、
  ブランチ名からハッシュ生成。main / master / develop はニュートラル色）。
- クリックでインライン展開: コミットメッセージ本文 + 変更ファイル
  一覧（NEW/MOD/REN/DEL バッジ、`+/−` 行数、バイナリは `bin` +
  サイズ、GitLens スタイルのファイル名強調）。
- 独立した diff ウィンドウ（シングルトン、再利用、位置・サイズ +
  最大化状態を永続化）でフル表示: ファイルサイドバー + 横スクロール
  同期の side-by-side diff。PNG / JPG / GIF / WebP / SVG の画像
  プレビュー内蔵（チェッカー背景、before / after）。ローカルの
  Git LFS オブジェクトは自動で参照。見つからない場合はインラインで
  説明。
- Copy as AI context — `c` キーまたはボタン — コミット、ファイル
  一覧、（十分小さければ）diff 全体を Markdown ブロックとして出力。
  Claude / Codex / Cursor にそのまま貼り付け可能。

## Diff ウィンドウ

*「待って、エージェントは本当にそれをやった?」* と思った瞬間のために。
任意のコミットをクリックすると独立したウィンドウが開く — フルの
ファイルサイドバー、横スクロール同期の side-by-side diff、バイナリ
アセットのインライン画像プレビュー、そして 位置・サイズ・最大化状態を
記憶するシングルトンインスタンス。

![diff window](docs/images/diff.gif)

## 技術スタック

Tauri 2 · Rust · React + TypeScript · `git2` · SQLite · カスタム SVG
DAG 描画 · テレメトリなし、フォンホームなし — ネットワーク利用は
オプトアウト可能なアップデート確認のみ。

## 開発

```bash
pnpm install
pnpm tauri dev
```

必要環境: Node 20+、Rust stable（Windows では msvc ツールチェーン）、
Visual C++ Build Tools（Windows）または Xcode CLT（macOS）。

## プラットフォーム

- Windows 10/11 — 主要ターゲット、開発機で検証済み
- macOS 13+ — 動作するはず、検証は少なめ
- Linux — 後日

## ライセンス

[MIT](LICENSE)
