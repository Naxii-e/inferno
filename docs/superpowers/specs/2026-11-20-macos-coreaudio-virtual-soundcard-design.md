# Inferno macOS 対応: CoreAudio 仮想サウンドカード 設計書

作成日: 2026-11-20
状態: ドラフト v2(ビルダーチェック反映済み、承認待ち)
対象読者: この設計から実装計画を立てて実装する人(この会話は読めない前提)

---

## 1. 目的と背景

Inferno は Dante プロトコルの非公式実装で、現状は Linux 専用である。音声バックエンドは ALSA
プラグイン(`alsa_pcm_inferno`)、クロック同期は作者フォーク版 Statime(Linux 専用デーモン)に依存する。

この設計の目的は、**macOS 上で Logic や Ableton などの DAW から Dante デバイスとして見える
仮想サウンドカード**を提供することである。Dante Virtual Soundcard(DVS)の代替を Inferno で実現する。

### 1.1 なぜ今か

事前調査(2026-11-20、macOS 27.0.1 / Apple silicon)で次が確認でき、実現性の見通しが立った。

- `inferno2pipe` は usrvclock-rs の 7 行修正だけで Mac でビルドできる。コアはほぼ移植済み。
- Statime フォークの OS 非依存部分(`statime` コア、`timestamped-socket`)は無修正で Mac でビルドできる。
  `clock-steering` は 2 箇所、`statime-linux` は 5 箇所の Linux 依存を直せばよい。
- coreaudiod のサンドボックス内から UDP、マルチキャスト、Unix ドメインソケット、`/tmp` 書き込みが
  できることを、実機に最小 HAL プラグインを入れて確認した(§7 参照)。
- 本物の DVS(DvsAudioPlugIn.driver 4.5.2)が同じ構成(HAL プラグイン内でネットワーク +
  root デーモン)で動いていることを同じ Mac で確認した。

---

## 2. ゴール / 非ゴール / アンチゴール

### 2.1 ゴール

1. macOS の「オーディオ装置」に Inferno デバイス(N 入力 / M 出力)が常時表示され、DAW が入出力に使える。
2. Dante Controller から Inferno デバイスが見え、既存 Linux 版と同じ範囲でルーティングできる。
3. DAW の再生/録音が Dante ネットワークのクロック(PTPv1)に従う。長時間運用でドリフトによる
   欠落や詰まりが起きない。
4. Linux 版の機能・ビルドを一切壊さない。macOS 固有コードは `cfg` または独立クレートに隔離する。
5. 開発者が自分の Mac にローカルビルド + ad-hoc 署名でインストールして使える。

### 2.2 非ゴール(今回はやらない)

- Windows 対応、AES67 対応、Dante Domain Manager 対応。
- Apple Developer ID による署名・公証、App Store 配布。
- ハードウェアタイムスタンプ(macOS には PHC がない)。
- PTP マスター動作(Mac を Dante のクロックリーダーにする)。将来 Statime の設定で可能になるかもしれないが対象外。
- GUI(設定アプリ、メーター表示)。設定はファイルで行う。
- 構成 B(薄いプラグイン + 別デーモンでサンプル精度 IPC)。構成 A(§4)で問題が出た場合の予備案として残す。

### 2.3 アンチゴール(こうなったら失敗)

- 実装のバグで Mac のシステム音声全体が止まる、または coreaudiod が再起動ループする。
- Linux ユーザーのビルドやテストが macOS 対応の副作用で壊れる。
- Dante ネットワーク上の他機器の動作を、Linux 版が与えない影響で乱す。

---

## 3. 制約

- 言語は Rust。プラグイン本体も Rust で書く(FFI 境界の薄い C は許容、§5.2)。
- ライセンス: inferno は GPL-3.0-or-later OR AGPL-3.0-or-later。Statime フォークは MIT OR Apache-2.0。
  BlackHole(GPL-3.0)のコードは**参照はしてもコピーしない**。
- 主ターゲットは macOS 27 / Apple silicon(開発者の実機)。x86_64 はベストエフォート。
- リンクには Xcode の SDK が必要。Command Line Tools 単体の SDK(MacOSX27.0.sdk)は `ld` が
  `arm64e.x1-macos` を解釈できずリンクに失敗する(確認済み)。ビルド手順で `xcrun --sdk macosx --show-sdk-path`
  を `-isysroot` に渡す。
- PTP デーモンは UDP 319/320 を使うため root(launchd)で動く。HAL プラグインは `_coreaudiod`
  ユーザーのサンドボックス内で動き、root にはなれない。
- HAL プラグインの vtable 関数内で panic してはならない(FFI 境界を越える unwinding は UB)。
  すべてのエントリで `catch_unwind` する。
- **リポジトリ方針(決定)**: 変更は開発者自身の GitHub フォーク(アカウント `Naxii-e`)で完結させる。
  対象は 3 つ: `inferno`(上流 `teodly/inferno`)、`usrvclock-rs`(上流 `gitlab.com/lumifaza/usrvclock-rs`、
  ブランチ `next`)、`statime`(上流 `teodly/statime`、ブランチ `inferno-dev`)。`.gitmodules` の usrvclock-rs の
  URL と README に書く Statime の clone URL は自分のフォークに向ける。上流への PR は将来の検討事項で、
  今回の作業の前提にはしない。現在このクローンの `origin` は上流そのものなので、マイルストーン 0 の最初の作業は
  フォーク作成と `origin` の付け替えになる。
- **検証環境(前提、確認済み)**: 開発 Mac と同じ LAN に PTPv1 リーダーになる Dante ハードウェアと
  Dante Controller があり、試験中は同じ Mac の DVS を停止できる。Linux ビルドの非破壊確認は、
  リポジトリに CI が無いため既存の Dockerfile(`Dockerfile.debian-alsa` など)で行う。

---

## 4. 全体構成(決定事項)

```
                          Mac
 ┌──────────────────────────────────────────────────────────────────┐
 │  launchd (root)                                                  │
 │  ┌──────────────────────────────┐   usrvclock overlay            │
 │  │ statime (macOS 版, PTPv1 従属) │──── Unix datagram ─────┐        │
 │  │  UDP 319/320, SW timestamps  │   /tmp/ptp-usrvclock   │        │
 │  └──────────────────────────────┘                        ▼        │
 │                                   ┌───────────────────────────────┐│
 │  DAW ──CoreAudio HAL──▶ coreaudiod│ Core Audio Driver             ││
 │                                   │ (Inferno.driver, _coreaudiod) ││
 │                                   │  ┌────────────────────────┐   ││
 │                                   │  │ coreaudio_hal_inferno  │   ││
 │                                   │  │  vtable / 時刻対 / 変換 │   ││
 │                                   │  ├────────────────────────┤   ││
 │                                   │  │ inferno_aoip           │   ││
 │                                   │  │ DeviceServer (mDNS,    │   ││
 │                                   │  │ ARC/CMC, flows RX/TX)  │◀──┼┼── Dante ネットワーク
 │                                   │  └────────────────────────┘   ││   (UDP 4455/8700/4400/8800/5353)
 │                                   └───────────────────────────────┘│
 └──────────────────────────────────────────────────────────────────┘
```

決定: **構成 A「太いプラグイン」**。`inferno_aoip` の `DeviceServer` を HAL プラグインのプロセス内で動かす。

根拠:
- 既存の ALSA プラグインがまったく同じ構成(アプリのプロセス内で DeviceServer を動かし、
  `ExternalBufferParameters` で外部バッファに直接書く)なので、設計を流用できる。
- macOS 27 では HAL プラグインは coreaudiod 本体ではなく、プラグインごとの独立プロセス
  `Core Audio Driver (X.driver)` で動く(確認済み)。クラッシュしてもシステム音声全体は巻き込まない。
- DVS 自身がこの構成(`AudioServerPlugIn_Network = true`、mach サービス宣言なし、root の `dvsd`)。
- サンドボックス内からの必要な IO がすべて通ることを実機で確認した(§7)。

PTP デーモンは別プロセス(root)である。これは特権ポートの都合であり、現在の Linux 構成
(`sudo statime` + usrvclock ソケット)と同じ分離線なので、Inferno 側の変更は最小になる。

---

## 5. コンポーネント設計

### 5.1 inferno_aoip / usrvclock-rs の移植(最小修正)

| 箇所 | 現状 | 変更 |
|---|---|---|
| `usrvclock-rs/src/lib.rs` `freq_scale_including_hw` | `#[cfg(target_os="linux")]` のみ | 非 Linux 用に `self.freq_scale` を返すフォールバックを追加(スパイクで動作確認済み、7 行) |
| `inferno_aoip/src/util/os.rs` `set_current_thread_realtime` | unix 共通で `SCHED_FIFO` | macOS 用実装を追加: `thread_policy_set(THREAD_TIME_CONSTRAINT_POLICY)`。`SCHED_FIFO` はサンドボックス内で `ENOTSUP`(確認済み)。失敗しても警告ログのみで続行する |
| `inferno_aoip/src/state_storage.rs` | `platform_dirs` の state_dir(ホーム配下) | `_coreaudiod` のホームは書けない。`Settings` に状態ディレクトリの上書き(`STATE_DIR`)を追加し、既定は従来どおり |
| `Settings::new` の設定取得 | `INFERNO_*` 環境変数 + `BTreeMap` | 変更なし。プラグイン側が設定ファイルを読んで `BTreeMap` に渡す(環境変数は coreaudiod では使えない) |
| IP アドレス選択 | 既存の設定キー `BIND_IP`(IP アドレスまたはインターフェース名)/ 未指定なら自動検出 | 変更なし。Mac は複数インターフェースが普通なので、サンプル設定ファイルで `BIND_IP` を明示する |

`ClockOverlay.clock_id` は macOS の `clockid_t` 値(§5.3)で、`nix::time::ClockId::from_raw` が
そのまま `clock_gettime` を呼ぶため、受信側のコード変更は不要。

### 5.2 新クレート `coreaudio_hal_inferno`(HAL プラグイン)

- `crate-type = ["cdylib"]`。ビルド成果物を `Inferno.driver/Contents/MacOS/Inferno` に配置し、
  `Info.plist`(`CFPlugInFactories`, `CFPlugInTypes`、**`AudioServerPlugIn_Network = true`**)と
  一緒にバンドルにする。バンドル化・ad-hoc 署名・`/Library/Audio/Plug-Ins/HAL/` へのインストール・
  coreaudiod 再起動はスクリプトで行う(`sudo` 必要)。
- Linux では `cargo build --workspace` に影響しないよう、ワークスペース `members` には入れず、
  `[workspace]` の `exclude` またはターゲット条件付きで扱う。**macOS 以外でビルドされないこと**。
- バインディング: `coreaudio-sys` には `AudioServerPlugIn.h` が含まれない。推奨は `build.rs` で
  bindgen を SDK ヘッダ `CoreAudio/AudioServerPlugIn.h` に対して実行する。bindgen が重い・不安定なら、
  ファクトリ関数と vtable だけを C の薄いシムで書き、Rust の `extern "C"` を呼ぶ形でもよい(実装者判断)。
- 実装する vtable は 23 関数(`AudioServerPlugInDriverInterface`)。オブジェクトモデルは
  PlugIn → Device(1 つ)→ Stream(入力 1、出力 1)→ Control(最小限、無くてもよい)。
  Box オブジェクトは任意(DVS には無い)。
- オーディオ形式: `kAudioFormatLinearPCM`、32-bit signed integer、インターリーブ。
  `inferno_aoip::device_server::Sample` が `i32` なので変換なしでコピーできる。HAL がプラグインに
  渡す IO バッファの形式はストリームの物理形式である旨をヘッダで確認し、Float32 しか受け付けない
  場合は Float32 と `i32` の変換を `DoIOOperation` で行う(実装者が確認)。
- サンプルレートは Dante ネットワークに合わせて設定ファイルで固定(`SAMPLE_RATE`、既定 48000)。
  利用可能レートとして 1 つだけ公開する。チャンネル数は `RX_CHANNELS` / `TX_CHANNELS`。
  変更にはプラグインの再読み込み(coreaudiod 再起動)が必要。これは ALSA 版と同じ制約。

#### 設定ファイルの場所

サンドボックスの文書上の記述は「バンドル内のみ読める」だが、実機では `/tmp` と `/Users/Shared`
への書き込みが通った。読み取り可能な場所の確定はマイルストーン 2 で検証する。優先順位:

1. `/Library/Application Support/Inferno/hal.toml`(読めれば)
2. バンドル内 `Contents/Resources/hal.toml`(常に読める。編集後に再署名が必要)

形式は TOML のフラットなキー=値で、キーは `Settings::new` が受け取る名前と同じ
(`NAME`, `SAMPLE_RATE`, `RX_CHANNELS`, `TX_CHANNELS`, `TX_LATENCY_NS`, `CLOCK_PATH`, `BIND_IP`, `DEVICE_ID`,
`STATE_DIR` など)。

#### ライフサイクル

| HAL 側イベント | プラグインの動作 |
|---|---|
| `Initialize` | 設定読み込み。`run_future_in_new_thread` で tokio ランタイムを立て、バックグラウンドで `DeviceServer::start` を試みる。CoreAudio 側のデバイスは DeviceServer の成否と無関係に即座に公開する(`Initialize` はブロックしない) |
| `StartIO`(最初のクライアント) | リングバッファを確保し `ExternalBufferParameters` に包む。`receive_to_external_buffer` と `transmit_from_external_buffer` を開始。`start_time` は §5.3 のアンカー |
| `DoIOOperation` ReadInput / WriteMix | `mSampleTime` を Dante タイムライン位置に変換し、リングバッファとの間でコピー(§5.3) |
| `StopIO`(最後のクライアント) | `stop_receiver` / `stop_transmitter`。バッファ解放 |
| プロセス終了 | `DeviceServer::shutdown`(ベストエフォート) |

#### ネットワーク未接続・IP 変化(決定)

coreaudiod は起動時に全プラグインを読み込むため、`Initialize` は DHCP 取得前や Wi-Fi 接続前に走り得る。
既存コードは IP が無いと `local_ip().expect(...)` などで panic し、`DeviceServer::start` も bind 失敗で落ち得る。

- プラグインは **DeviceServer 監督タスク**を持つ。`Settings::new` と `DeviceServer::start` を
  `catch_unwind` で包み、失敗したら指数バックオフ(1 秒から最大 30 秒)で再試行し続ける。
  成功するまで Dante 側には何も現れないが、CoreAudio 側のデバイスは存在し、`StartIO` は無音で動く。
- DeviceServer 起動後の **IP 変化(Wi-Fi⇄Ethernet、DHCP 更新、スリープ復帰)は v1 では追従しない**。
  変化を検出したらログに警告し、README に「ネットワーク構成を変えたら coreaudiod を再起動する」と書く。
  ALSA 版も追従していない。追従(停止→フロー破棄→再起動、IO 中は無音継続)は将来課題。
- 既定の `DEVICE_ID` は IP 由来なので、DHCP で IP が変わるたび Dante 上で「別デバイス」になり、
  購読と名前が失われる。サンプル `hal.toml` では **`DEVICE_ID` を明示(必須扱い)**し、
  インストール手順でランダム生成した値を書き込む。

ALSA 版と同じく、IO 停止中は Dante のフローが切れ再接続に数秒かかる。デバイス自体は
coreaudiod が生きている限り存在するため、ALSA 版の「アプリを閉じるとデバイスが消える」問題は無い。
フローを IO 停止中も維持する改良は将来課題(非ゴール)。

### 5.3 クロック設計(本設計の核心)

#### 用語

- **Dante メディアクロック**: PTP 時刻をサンプルレートで割ったサンプル番号。`inferno_aoip` の
  `MediaClock::now_in_timebase(rate)`。
- **オーバーレイ**: usrvclock が配る `(clock_id, last_sync, shift, freq_scale)`。
  `overlay_ns(t) = t + shift + (t - last_sync) * freq_scale`、`t` は基準クロックの ns。
- **ホスト時刻**: CoreAudio が使う `mach_absolute_time()` のティック。

#### 基準クロックの決定

実機測定で `mach_absolute_time()` は `CLOCK_UPTIME_RAW` と一致(差 300 ns 程度)、
`CLOCK_MONOTONIC_RAW` は `mach_continuous_time()`(スリープ中も進む)、`CLOCK_MONOTONIC` は
NTP の周波数補正を受けて RAW 系から 12 ms ずれていた。

決定: **PTP デーモンのオーバーレイ基準クロックは `CLOCK_UPTIME_RAW`(clockid 8)**。
理由: (1) CoreAudio のホスト時刻と無変換で対応する、(2) NTP の干渉を受けない
(Linux README が問題視している衝突を構造的に回避する)、(3) スリープ復帰時は PTP が再同期するので
継続時間を含める必要がない。

変換: `ticks = ns * timebase.denom / timebase.numer`(Apple silicon では 125/3)。
`mach_timebase_info` は起動時に 1 回取得する。

#### 時刻対(GetZeroTimeStamp)

HAL はドライバが報告する「(サンプル時刻, ホスト時刻) の対」の列からデバイスクロックの速度を推定し、
クライアントの IO をそれに追従させる。したがって**真実の対を返すだけで DAW は Dante クロックに従う**。
ALSA 版の `pointer()` コールバック(メディアクロックの現在値を返す)に相当する役割を、こちらは
「次に来るリングバッファ境界の時刻を予告する」形で果たす。

- アンカー: `StartIO` 時に `S0 = MediaClock::now_in_timebase(rate)`(Dante タイムラインのサンプル番号)。
  デバイスのサンプル時刻 `s` は Dante 位置 `S0 + s` に対応する。この `S0` を
  `receive_to_external_buffer` / `transmit_from_external_buffer` の `start_time` にも渡す。
- 時刻対の周期 `P`(`kAudioDevicePropertyZeroTimeStampPeriod`)はリングバッファ長と同じにする。
  初期値は 4096 フレーム(実装者が調整可)。
- `GetZeroTimeStamp` は、現在のホスト時刻以降で最初の `s = k * P` を求め、
  `host(s) = overlay⁻¹((S0 + s) / rate * 1e9)` をティックに変換して返す。
  逆変換: `t = last_sync + (T - shift - last_sync) / (1 + freq_scale)`。
- **seed**: オーバーレイの更新で `host(s)` が前回の予告から閾値(初期値 1 ms)以上動いたら
  seed をインクリメントして HAL に再同期させる。小さな変化は seed を変えず、次の時刻対で自然に吸収させる。
- オーバーレイがまだ無い(PTP デーモン未起動、または PTP 未ロック)状態で `StartIO` された場合(決定):
  アンカー `S0` は確定できないので **「アンカー未確定」状態**に入る。時刻対は `CLOCK_UPTIME_RAW` の
  フリーランで返し、IO は無音で動かし、`s = 0` に対応するホスト時刻 `H0` を記録する。
  最初のオーバーレイが届いたら `S0 = overlay(H0) * rate / 1e9` を遡って決め、`start_time` の oneshot で
  送受信側へ渡して Dante フローに合流させる。同時に seed を進めて HAL を再同期させる。IO は止めない。
  つまり DAW を開いたまま PTP デーモンを起動しても音が出るようになる。
- IO 中にオーバーレイが途切れる・跳ぶ(デーモン再起動など)場合: `s ↔ S0 + s` の対応は固定したまま、
  新しいオーバーレイで時刻対を計算し直し、閾値を超えていれば seed を進める。グリッチ 1 回で整合が戻る。

#### バッファとレイテンシ

- リングバッファは `coreaudio_hal_inferno` が所有する `i32` のインターリーブ配列
  (長さ `P * channels`、`ExternalBufferParameters::new(ptr, len, stride=channels, valid, None)`)。
  ALSA 版が ALSA の mmap 領域を包んでいるのと同じ使い方。
- 出力(DAW → Dante): HAL は `mSampleTime` が未来のサンプルをくれる。`flows_tx` は
  メディアクロック現在値から `tx_latency_ns` 分遅れた位置を読む。HAL が書き込む先行量が
  常に `tx_latency` 以上になるよう、`kAudioDevicePropertySafetyOffset`(出力)を
  `tx_latency_ns` 相当以上に設定する。
- 入力(Dante → DAW): `flows_rx` はパケット到着に合わせて Dante 位置に書き、`readable_pos` を進める。
  HAL は `mSampleTime` が過去の入力を読む。入力の `SafetyOffset` と `Latency` を Dante 受信
  レイテンシ(`DeviceInfo.latency_ns`)相当に設定する。
- 未到着領域を読んだ場合は無音を返し、`ring_buffer` の穴カウンタで記録する(xrun 相当)。
  ALSA 版のように送受信を再起動しない。

### 5.4 PTP デーモン(Statime フォークの macOS 対応)

作業場所は Statime フォーク(`teodly/statime` の `inferno-dev` ブランチをさらにフォーク)。
Inferno リポジトリには README への手順と launchd の plist、設定ファイルのサンプルを置く。

事前調査で判明した修正範囲:

| クレート | 状態 | 修正 |
|---|---|---|
| `statime`(コア) | 無修正でビルド可 | なし |
| `timestamped-socket` | 無修正でビルド可。`socket/macos.rs` があり `SoftwareRecv`(`SO_TIMESTAMP`、µs 精度)のみ対応 | なし。送信タイムスタンプは非対応なので上位で代用 |
| `clock-steering` | `unix.rs` の `fd` フィールド 2 箇所と `CLOCK_TAI` 定数が Linux 限定 | `cfg` を付ける(スパイクで確認済み) |
| `statime-linux` | 5 エラー: `CLOCK_TAI`(2)、`UnixClock::open`(/dev/ptp)、`system_offset`、`open_interface_udp4/6/ethernet` と `EthernetAddress/MacAddress` の import | macOS 版の `clock` / `socket` モジュールを `cfg` で分ける |

設計上の決定:

- クロック構成は `inferno-ptpv1.toml` と同じ `virtual-system-clock = true` を使う。カーネル時計は触らない。
  `virtual-system-clock-base` に新しい値 `uptime_raw`(= `CLOCK_UPTIME_RAW`)を追加し、macOS 用の
  設定ファイル `inferno-ptpv1-macos.toml` ではこれを使う。
- 送信タイムスタンプ: `sendto` が返った直後に `CLOCK_UPTIME_RAW` を読んで代用する。
  受信は `SO_TIMESTAMP`(`CLOCK_REALTIME` ベースの timeval)なので、受信時に `CLOCK_REALTIME` と
  `CLOCK_UPTIME_RAW` の差を取って基準クロックへ換算する(差は毎回測り直す。NTP で REALTIME が動くため)。
- ソケットは `open_ip`(アドレス bind)+ `join_multicast(interface)` を使う。Ethernet(raw)モードは非対応。
- `usrvclock-export = true`、ソケットパスは既定の `/tmp/ptp-usrvclock`。usrvclock の `Server::new`
  がパーミッションを緩めるので `_coreaudiod` から接続できる(実機で `_coreaudiod` からの接続を確認済み)。
- 配布: `/Library/LaunchDaemons/org.inferno.ptp.plist`、バイナリは `/usr/local/libexec/inferno/statime`
  (パスは実装者判断)。`RunAtLoad`、`KeepAlive`。

精度の見込み: ソフトウェアタイムスタンプなので数十 µs のジッタがある。Dante の受信バッファ
(`tx_latency_ns` 既定 10 ms、受信も同程度)に対して十分小さい。DVS も同じ土俵である。
**これが残る最大の不確定要素**で、マイルストーン 1 で実測して判断する(§6)。

### 5.5 ログ

サンドボックス内プロセスの `os_log` 既定レベルは `log show` で取り出せなかった(確認済み)。
決定: `env_logger` の出力先をファイル(`/tmp/inferno-hal.log`、書けることを確認済み)にし、
あわせて `os_log` の error レベルにも出す。ログレベルは設定ファイルの `LOG_LEVEL`。

---

## 6. マイルストーン

各マイルストーンは独立に検証できる成果物を持つ。順序は依存関係順。

| # | 成果物 | 完了条件 |
|---|---|---|
| 0 | inferno リポジトリが macOS でビルドできる | `cargo build -p inferno2pipe` が Mac で通る。usrvclock-rs の修正、`os.rs` の macOS 実装、`STATE_DIR`、`IP` 設定。Linux ビルドとテストが従来どおり通る(CI または手動確認) |
| 1 | PTP デーモンの macOS 版 | Mac 上で Statime が Dante 機器の PTPv1 に従属し、オーバーレイを配信する。`inferno2pipe` が Dante 機器から録音でき、1 時間でサンプル欠落や詰まりがない。オフセットの揺れを記録し、設計の前提(ms 未満)を確認 |
| 2 | HAL プラグインの骨格 | デバイスが「Audio MIDI 設定」に表示され、N 入力 / M 出力。DAW から選べる。IO は無音と破棄。時刻対は `CLOCK_UPTIME_RAW` フリーラン。バンドル化・署名・インストールのスクリプト。設定ファイルの読み取り可能な場所の確定。サンドボックス内で `thread_policy_set` が通るかの確認 |
| 3 | Dante 音声の接続 | RX(Dante → DAW 入力)で DAW が録音できる。次に TX(DAW → Dante)。Dante Controller から Inferno が見えルーティングできる |
| 4 | クロック従属 | オーバーレイから時刻対を生成。seed による再同期。`SafetyOffset` と `Latency` の調整。1 時間以上の再生/録音でドリフトによる欠落なし |
| 5 | 配布と文書 | README に macOS 節(前提、ビルド、インストール、既知の制限)。PTP デーモンの launchd plist。アンインストール手順 |

判断点は 2 つ。マイルストーン 1 で PTP 精度が足りない場合、マイルストーン 3 で mDNS の競合が解けない場合は、
そこで止めて設計を見直す(構成 B、別のクロック伝達方式、dns-sd ベースの mDNS など)。
マイルストーン 0 にはフォーク作成と `origin` / `.gitmodules` の付け替えを含める(§3 リポジトリ方針)。

---

## 7. 事前検証の記録(2026-11-20)

実機(macOS 27.0.1、Apple silicon)で、デバイスを公開しない最小 HAL プラグイン(C、ad-hoc 署名)を
`/Library/Audio/Plug-Ins/HAL/` に入れ、`Initialize` 内で試した結果。プラグインは
`Core Audio Driver (InfernoProbe.driver)` という独立プロセス(ユーザー `_coreaudiod`)で動いた。

| 項目 | 結果 |
|---|---|
| UDP ソケット作成、ポート 4455 bind | OK |
| マルチキャスト参加(224.0.0.251)と送信 | OK |
| localhost UDP 送信 | OK |
| Unix ドメインソケット bind(`$TMPDIR` と `/tmp`) | OK |
| `/tmp/...` の Unix datagram サーバへの connect と send | OK(受信側で確認) |
| `/tmp` と `/Users/Shared` へのファイル書き込み | OK |
| `pthread_setschedparam(SCHED_FIFO)` | 失敗(ENOTSUP)。通常プロセスでは成功 |
| `thread_policy_set(THREAD_TIME_CONSTRAINT_POLICY)` | 通常プロセスで成功。サンドボックス内は未検証 |
| `os_log` 既定レベルの `log show` での取得 | 失敗(残らない) |
| ad-hoc 署名のローカルビルドの読み込み | OK |

同じ Mac にインストール済みの DVS の構成: `AudioServerPlugIn_Network = true`、
`AudioServerPlugIn_MachServices` なし、root の `dvsd`(launchd)。NDI Audio ドライバも同じ宣言。

Statime フォーク(`inferno-dev`)を Mac でビルドした結果は §5.4 の表のとおり。

---

## 8. リスクと未検証事項

| リスク | 影響 | 対処 |
|---|---|---|
| ソフトウェアタイムスタンプの PTP 精度不足 | 同期が揺れて欠落 | M1 で実測。不足なら設計見直し |
| mDNS ポート 5353 で mDNSResponder と競合(Dante Controller からのユニキャスト問い合わせが片方にしか届かない可能性) | デバイスが Dante Controller に出ない | searchfire は `SO_REUSEADDR` と `SO_REUSEPORT` を使う。M3 で確認。**競合が判明したら M1 の精度不足と同じ「判断点」として止め、設計を見直す**(dns-sd API ベースの差し替えは `mdns_server.rs` と `mdns_client.rs` 両方に及ぶ規模で、この計画には含めない) |
| `thread_policy_set` がサンドボックスで拒否 | RX/TX スレッドの遅延 | M2 で確認。拒否なら HAL の IO スレッドに処理を寄せる設計へ |
| 設定ファイルの場所が読めない | 設定できない | バンドル内 Resources にフォールバック |
| HAL が渡す IO バッファ形式が Float32 固定 | 変換コスト | `DoIOOperation` で変換。性能上は問題ない |
| 同じ Mac 上の DVS と Inferno のポート衝突 | 開発機でのテストが面倒 | Inferno 側は `ALT_PORT` で回避可能。PTP は両者とも従属なら 319/320 を共有できるかは未検証。確実なのは DVS を止めてテスト |
| 複数 NIC での IP 選択ミス | 別セグメントに広告 | `IP` 設定で明示 |

---

## 9. 実装者に委ねる事項

- bindgen か C シムかの選択(§5.2)。
- `coreaudio_hal_inferno` 内のモジュール分割、オブジェクト ID の割り当て、プロパティ実装の網羅範囲。
- リングバッファ長 `P`、seed 更新の閾値、`SafetyOffset` の具体値。設計の初期値から始めて計測で調整する。
- 単体テストの範囲。少なくともオーバーレイ⇄ホスト時刻の変換(逆変換の往復、ラップ、`freq_scale` の
  符号)と、`mSampleTime` ⇄ Dante 位置の変換は純粋関数として切り出してテストする。
- 開発用スクリプト群(バンドル化、署名、インストール、ログ閲覧、`DEVICE_ID` 生成)の形。
- `STATE_DIR` の既定位置、ワークスペースからの除外方法、x86_64 の扱い。
- Statime フォーク側のモジュール構成(`statime-linux` 内に `cfg` で同居させるか、`statime-macos`
  クレートに分けるか)。

---

## 10. 参考(リポジトリ内の該当箇所)

- 既存の「プロセス内 DeviceServer + 外部バッファ」の実装例: `alsa_pcm_inferno/src/lib.rs`
  (`plugin_pointer`, `plugin_prepare`, `plugin_start`)
- 外部バッファ API: `inferno_aoip/src/ring_buffer.rs`(`ExternalBufferParameters`, `wrap_external_source`,
  `wrap_external_sink`)、`inferno_aoip/src/device_server/mod.rs`(`receive_to_external_buffer`,
  `transmit_from_external_buffer`, `get_realtime_clock_receiver`)
- メディアクロックとオーバーレイ受信: `inferno_aoip/src/media_clock.rs`、`usrvclock-rs/src/lib.rs`
- 設定キー: `inferno_aoip/src/device_server/settings.rs`
- RT スレッド: `inferno_aoip/src/util/os.rs`(呼び出し元は `flows_rx.rs:195`, `flows_tx.rs:275`)
- Statime フォーク: https://github.com/teodly/statime (branch `inferno-dev`)、設定例 `inferno-ptpv1.toml`
- Apple の制約の一次資料: SDK 内 `CoreAudio/AudioServerPlugIn.h` 冒頭コメント(サンドボックス、
  `AudioServerPlugIn_Network`, `AudioServerPlugIn_MachServices`)
