# Rovta

ターミナルで動く、コード駆動型の音楽制作環境。TUIでの視覚的な編集と TypeScript による楽曲記述を、**同じテキストファイル**で双方向に扱います。

```
TypeScript → 型除去(JS) → QuickJS-NG → Song IR → Rust音楽エンジン → CPAL出力 / WAV
```

| 項目 | 実装 |
|---|---|
| アプリ | Rust（`src/`） |
| TUI | ratatui + crossterm |
| 音楽記述 | TypeScript（`src/ts.rs` が型を除去。行・桁は保持されるのでエラー位置が一致） |
| 実行環境 | QuickJS-NG（rquickjs 0.9）、ES Modules による複数ファイル構成 |
| 音声 | CPAL。シンセ(sine/saw/square/tri, PolyBLEP)・ドラム音源・サンプラー(WAV)・SVFフィルタ・ドライブ・ピンポンディレイ・リバーブ・オートメーション |
| データ | 独自 Song IR（`src/ir.rs`）。プロジェクトはテキストのみ（独自バイナリ形式なし） |

## ビルドと起動

```sh
# Linux: sudo apt install libasound2-dev pkg-config clang   （最近の stable Rust が必要）
cargo build --release
./target/release/rovta my-song        # 空なら starter プロジェクトを生成して起動
./target/release/rovta my-song --render out.wav    # ヘッドレスWAV書き出し
./target/release/rovta my-song --midi out.mid      # ヘッドレスMIDI書き出し
./target/release/rovta my-song --check             # スクリプトの実行チェックのみ
```

## 操作（キーボードのみ）

**共通**: `Ctrl+S` 保存（CODEでは保存＋実行） / `Ctrl+Q` 終了 / `Tab`(TUI→CODE) / `Esc`(CODE→TUI)。**ファンクションキーは一切使いません**

**TUI**: `Space` 再生/停止 · `p` カーソル位置から再生 · `Home` 巻き戻し · `1` ピアノロール `2` ステップ `3` ミキサー ·
`[` `]` トラック選択 · `n`/`N` シンセ/ドラムトラック追加 · `Ctrl+D`×2 削除 · `<` `>` BPM · `T` 拍子 ·
`e` WAV書き出し · `m` MIDI書き出し · `i` MIDI取り込み（`out/` に出力）

- 元に戻す: `u` / `Ctrl+Z`、やり直し: `U` / `Ctrl+Y`（ノート・ミキサー・トラック追加削除・BPM・MIDI取り込みが対象。コードを編集すると履歴はリセット）
- ピアノロール: 矢印移動 / `Enter` ノート入力・削除 / `l` `L` 長さ / `v` `V` ベロシティ / `g` グリッド / `PgUp` `PgDn` 小節移動 / `Shift+↑↓` オクターブ
- ステップ: 1小節16ステップ。`Enter`/`x` で切替。ドラムはレーン表示
- ミキサー: `←→` トラック `↑↓` パラメータ `+`/`-` 調整 `PgUp/PgDn` 粗調整 `m` ミュート `s` ソロ（音量・パン・音色・カットオフ・ドライブ・ディレイ・リバーブ・ADSR）

**CODE**: 行番号・構文色・エラー行表示（ガターに `!`）・コンソール出力パネル / `Ctrl+Space`(`Ctrl+N`) 補完 /
`Ctrl+O` / `Ctrl+T`（または `Alt+←→`）ファイル切替 / `Ctrl+R` 実行 / `Ctrl+G` 再生・停止 / `Ctrl+P` ファイル作成 / `Ctrl+Z` 元に戻す / `Ctrl+/` コメント / `Ctrl+K` 行削除 / `Ctrl+D` 行複製

## 双方向同期の仕組み

TUIが管理するトラックは、ソース内の **領域** として書かれます。

```ts
// @tui track:bass
track("bass", { inst: "saw", vol: 0.7, cutoff: 900 })
  .notes([
    [0, 0.4, 33, 110],   // [開始beat, 長さbeat, ピッチ, ベロシティ]
  ]);
// @end
```

- **TUI→コード**: 編集のたびに該当領域だけを再生成します（領域外は1バイトも触りません）。
- **コード→TUI**: 実行（`Ctrl+S`/`Ctrl+R`/CODEを離れた時）のたびに結果のIRを読み直します。
- **高度なコードの保持**: 領域の中身が正規形と一致する間だけTUIが書き換えます。ループ・関数・コメントなどを書き足した領域、
  領域の外で定義したトラック（`*` 表示）は**コード所有**になり、再生はされますがTUIは決して上書きしません
  （ミュート/ソロのみセッション限定で可能）。

## スクリプトAPI（`rovta.d.ts` に型定義あり）

```ts
bpm(120); timeSig(4, 4);
track("lead", { inst: "saw", vol: 0.7, cutoff: 2400, delay: 0.3, reverb: 0.2 })
  .note(0, 1, "C4", 100)
  .notes([[1, 1, "E4"], [2, 2, "G4", 90]])
  .chord(4, 2, chord("C4", "maj7"))
  .steps("x...x...x.x.x...", 36, { step: 0.25 })     // x=ヒット X=アクセント .=休符
  .auto("cutoff", [[0, 400], [8, 6000]]);              // オートメーション（beat, 値）
n("C#4"); scale("A3", "minor", 2); chord("A3", "min7");
```
サンプラー: `track("kick", { inst: "sampler", sample: "samples/kick.wav", root: 60 })`

## 既知の制限（正直なところ）

- **型チェックはしません**。型は除去して実行するだけです（`tsc --noEmit` を別途併用してください）。`enum` / `namespace` / デコレータ / クラスのパラメータプロパティは非対応。
- オートメーションはコードで記述・再生でき、ミキサーに表示されますが、TUI上での図形編集は未実装です。
- MIDIは**ファイルの入出力のみ**（リアルタイムMIDI入力デバイスは未対応）。
- 実際の音出しはこの開発環境にオーディオデバイスが無いため未確認です。DSPはオフラインレンダリングで検証済み（ピーク0.77・クリップなし・NaNなし）。実機での再生はお手元で確認してください。
