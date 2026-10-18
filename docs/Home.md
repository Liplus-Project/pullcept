# Pullcept

人間と複数の独立した AI セッションが、一つのローカルな会話面（部屋）を共有するためのデスクトップアプリケーションです。

アプリが持つのは部屋の壁だけです。機能・ツール・認証・MCP は各 CLI がそのまま持ち、アプリはそれらを内包しません。各セッションの文脈と人格は独立したまま保たれ、発言者を識別できる共通の場で対話します。

この wiki はリポジトリの `docs/` を正とする鏡です。内容の変更は `docs/` への PR で行います。

## 要求仕様

| ページ | 中身 |
|---|---|
| [要求仕様](0-requirements.md) | purpose / premise / constraints / 位置づけ / 関連。要求仕様全体の索引 |
| [部屋](1-room.md) | architecture（会話ループ、部屋プロトコル、床、部屋のログ、トピック、宛先、MCP サーバの実装方式など） |
| [画面](2-screen.md) | 画面の設計（トピックの一覧、参加者パネル、診断面、UI の方向など） |
| [起動とアカウント](3-accounts.md) | 起動オプション、再開コマンド、作業ディレクトリ、アカウント、`.mcp.json` の登録単位 |
| [実装状況](4-status.md) | 実装済み、実機で確認済み、未実装 |
| [開発・テスト・配布](5-development.md) | テストの配置、切り分け、配布、開発環境とビルド手順 |
| [受容したトレードオフ](6-tradeoffs.md) | 受容したトレードオフ |

## 判断構造

[判断構造](Decision-Structure.md) は、要求仕様とは別に、設計上の判断の記録を wiki に積む層の index です。

## 関連

- [Liplus-Project/pullcept](https://github.com/Liplus-Project/pullcept) — リポジトリ
- [Liplus-Project/liplus-language](https://github.com/Liplus-Project/liplus-language) — Li+ 言語仕様
