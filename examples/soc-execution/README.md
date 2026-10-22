# 共有SoC実行例

未commit・未公開の作業例。`soc-execution.ini`は実RV32I programが42をstore/loadし、有限cache/directory、実AXI/AHB/NoC、coherent DMA、leased slot、AXI5 AtomicLoad ADDを同じMemoryServiceへ接続する。CPUはECALLでprecise trap、consumerは42、最後のobserveは43を返す。

`ddr.ini`は選択ISSI modeの実初期化後にAXI byte書込み、AHB読出し、AXI5 atomic、NoC DMAを実行する。DDRにpreloadを与えない。実halfcycle WriteByteだけが共有byteを変更し、R/B/HREADYはbackend完了を待つ。冷間初期化を含むためtime limitは730us。DDR maintenanceはhorizonまで継続し、未完了を補完しない。

production CLIで`dir-simulator validate --config <ini>`、`dir-simulator run --config <ini> --output <新規出力先>`を使う。Viewerは生成results.jsonを開く。CLI引数はその時点の`--help`を正本とする。

`mailbox.ini`は64byteのmapped registerから実slotをlease/publish/参照開始/releaseする。AXIの実4byte single beatでpayloadを公開し、consumerのNoC DMAが実byteをSRAMへcopyする。`observe-dma-copy`は`[7,8,9,10]`、最後のSTATUSはFreeを返す。制御register用に別のpayload RAMは作らない。

この例のconsumer DMAは通常の共有RAM読出しであり、slotの自動pinを行わない。例のRELEASEはDMAの実完了後に配置する。lease付きRequestのactive read保護とは区別する。
