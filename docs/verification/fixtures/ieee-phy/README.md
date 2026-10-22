# 選択IEEE PHYの出典と独立照合

文書ID：`verification-ieee-phy-fixtures`

[source-manifest.json](source-manifest.json)は規格の版・URL・ローカル入力のhash、採用option、製品上の境界、実装継続項目を記録する。規格本文は配布しない。`in_progress`の項目を実装完了や規格認証の証拠として扱わない。

1000BASE-T1のRS parity referenceは、Rust実装とは異なるPythonのroot-equation solverから生成する。T1SとT10のnetwork checkerは、既知のVLAN10/空payloadのframeを構築し、Python `zlib.crc32`でFCSを計算して実sampleから復号したbyte列と照合する。送信要求の元byte列を期待値として読み込まない。

```sh
python3 docs/verification/fixtures/t1s-network/verify_result.py tmp/t1s/results.json
python3 docs/verification/fixtures/ieee-phy/verify_t10_result.py tmp/t10/results.json
python3 docs/verification/fixtures/ieee-phy/verify_t10_result.py tmp/repeater/results.json --repeater
```

T10 checkerは一attemptにつきpreamble/SFD/FCS markerがそれぞれ一つであること、10Mb/sの64bit preamble/SFD区間と68B MAC区間を確認する。callbackの実行時刻とは別に、sample boundaryのmarkerを使う。これは古い実行結果の二重FCS markerを検出した検査であり、失敗を許容するための補正を含まない。
