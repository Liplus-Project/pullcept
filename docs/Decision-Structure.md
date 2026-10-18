# 判断構造（Decision Structure）

判断構造は、要求仕様（0-6）とは別の用途を持つ層である。設計上の分岐で何をなぜ選んだか、試して失敗した原因、検証した前提の結果を、セッションをまたいで再現できる形で残す。

**実体エントリ（`<topic>.md` 形式の kebab-case ファイル名）は GitHub Wiki に置く。** 本ファイル（`docs/Decision-Structure.md`）はその index と運用仕様だけを持ち、エントリ本体は持たない。

## 位置づけ

- 履歴（log）ではなく構造である。エントリは判断の現在の状態を書き、置き換わった判断は更新または supersede で表す。
- エントリは wiki へ直接書く（PR は通さない）。本ファイルの index 行は通常の PR フローで足す。
- wiki は `docs/` の鏡だが、判断構造のエントリだけは wiki 側が正である。

## エントリの書式

- ファイル名は小文字の kebab-case（`<topic>.md`）。順序の接頭辞は付けない。
- 順序はこの index と wiki のサイドバーで明示する。

## index

各行は wiki のエントリを絶対 URL（`https://github.com/Liplus-Project/pullcept/wiki` の下のページ）で指す。

まだエントリは無い。
