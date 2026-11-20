# M1: Statime フォークの macOS 移植(PTPv1 従属デーモン) 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `teodly/statime` の `inferno-dev` ブランチを macOS でビルド・実行できるようにし、Dante 機器の PTPv1 に従属して `/tmp/ptp-usrvclock` にクロックオーバーレイを配信する `statime` バイナリを作る。最後に Mac 上の `inferno2pipe` で Dante 機器から 1 時間録音して精度を実測する(設計の判断点)。

**Architecture:** OS 非依存の `statime` コアはそのまま使う。`clock-steering`、`timestamped-socket`、`statime-linux` の 3 クレートに `#[cfg(target_os = "macos")]` の実装を足す。クロックはカーネルを触らない「仮想システムクロック(DualLayer)」構成を流用し、基準クロックに `CLOCK_UPTIME_RAW` を追加する。受信タイムスタンプは `SO_TIMESTAMP`(µs、CLOCK_REALTIME 系)、送信タイムスタンプは送信直後のクロック読み取りで代用する。

**Tech Stack:** Rust、tokio、`libc`(`IP_BOUND_IF`, `IP_MULTICAST_IF`, `ip_mreq`, `ipv6_mreq`, `CLOCK_UPTIME_RAW`)、usrvclock-rs(Plan M0 の Task 2 で非 Linux 対応済みのフォーク)。

**Spec:** `docs/superpowers/specs/2026-11-20-macos-coreaudio-virtual-soundcard-design.md`(§5.4、§5.3 基準クロック、§6 M1、§8)

## Global Constraints

- 作業リポジトリは Naxii-e のフォーク 3 つ: `statime`(ブランチ `inferno-dev-macos`)、`clock-steering`(`from_clockid-macos`)、`timestamped-socket`(`inferno-dev-macos`)。上流 `teodly/*` には push しない。
- Linux ビルドを壊さない。macOS 固有コードは `cfg(target_os = "macos")` の中、Linux 固有の既存コードは `cfg(target_os = "linux")` で囲む。各タスクの最後に Docker で Linux ビルドを確認する(`docker run --rm -v "$PWD":/w -w /w rust:1.92.0-slim-trixie cargo build -p statime-linux`)。
- macOS では **カーネルの時計を一切変更しない**。`virtual-system-clock = true` 以外の設定は起動時エラーにする。
- 基準クロックは `CLOCK_UPTIME_RAW`(設定値 `"uptime_raw"`)。
- 作業ディレクトリは inferno と並ぶ `/Users/naxdev/Desktop/Devs/Rust/statime`。
- コミットメッセージは英語、末尾に `Co-Authored-By: Claude Mythos 5.1 <noreply@anthropic.com>`。

## Review Focus

1. **起動時にインターフェースに IPv4 アドレスが無い**(DHCP 前): ソケット作成が失敗して終了するのは許容するが、エラーにインターフェース名が含まれること。→ Task 3 のテスト `open_udp4_on_missing_interface_names_it`。
2. **macOS で `virtual-system-clock = false`**: システムクロックを root で操作してはならない。設定検証で明確なエラー終了。→ Task 4 のテスト `macos_requires_virtual_system_clock`。
3. **macOS で `network-mode = "ethernet"` や `hardware-clock = "required"`**: 未対応を明示して終了する。→ Task 4 のテスト `macos_rejects_ethernet_and_required_hw_clock`。
4. **受信パケットに `SCM_TIMESTAMP` が付かない**: 既存の `Missing recv timestamp` ログの連打になる。`SO_TIMESTAMP` 有効化が失敗したら起動時にエラーにする。→ Task 3 のテスト `recv_timestamp_present_with_software_recv`。
5. **`CLOCK_REALTIME` が NTP でステップした瞬間**: 中間クロックタスクが遅延 1 ms 超の測定を捨て、`ClockExportGuard` が 1 ms 超の跳びを 5 秒保留する既存機構で吸収される。macOS で基準を `CLOCK_UPTIME_RAW` にした構成でもこの経路が有効であること(ガードが `TAI` 以外で有効になる条件を変えない)。→ Task 4 の実装ステップで条件式をそのまま維持し、Task 6 の実測で `GUARD` ログを確認。

---

### Task 1: フォーク作成とクローン

**Files:**
- Create: `/Users/naxdev/Desktop/Devs/Rust/statime`(クローン)
- Modify: `statime/.gitmodules`(2 つのサブモジュール URL)

**Interfaces:**
- Produces: 3 つのフォークとブランチ。以降のタスクはすべてこのクローン内で作業する。

- [ ] **Step 1: 3 リポジトリをフォーク**

```bash
gh repo fork teodly/statime --clone=false
gh repo fork teodly/clock-steering --clone=false
gh repo fork teodly/timestamped-socket --clone=false
```

- [ ] **Step 2: クローンしてブランチを作る**

```bash
cd /Users/naxdev/Desktop/Devs/Rust
git clone --recurse-submodules -b inferno-dev https://github.com/Naxii-e/statime
cd statime && git checkout -b inferno-dev-macos
git -C clock-steering checkout -b from_clockid-macos
git -C timestamped-socket checkout -b inferno-dev-macos
```

- [ ] **Step 3: サブモジュール URL をフォークに向けて push**

`.gitmodules` の 2 つの `url` を `https://github.com/Naxii-e/timestamped-socket` と `https://github.com/Naxii-e/clock-steering` にし、`git submodule sync`。各サブモジュール内で `git remote set-url origin` を同じ URL にして `git push -u origin <branch>`。親で `git add .gitmodules && git commit -m "chore: point submodules at Naxii-e forks" && git push -u origin inferno-dev-macos`。

- [ ] **Step 4: macOS での現状を記録**

Run: `cargo build -p statime && cargo build -p statime-linux 2>&1 | grep -c "^error"`
Expected: コアはビルド成功、`statime-linux` は `clock-steering` の 2 エラーで停止(Task 2 で解消)。

---

### Task 2: clock-steering の macOS 対応

**Files:**
- Modify: `clock-steering/src/unix.rs`(`UnixClock::from_clockid` 18-23 行、`CLOCK_TAI` / `CLOCK_MONOTONIC` 定数 60-70 行付近)

**Interfaces:**
- Produces: macOS で `UnixClock::from_clockid(libc::clockid_t)`, `UnixClock::CLOCK_REALTIME`, `UnixClock::CLOCK_MONOTONIC` が使える。`UnixClock::CLOCK_TAI` と `UnixClock::open` は Linux 限定のまま。

- [ ] **Step 1: 失敗するテスト(`unix.rs` の `mod tests`、`#[cfg(target_os = "macos")]`)**

```rust
#[test]
fn uptime_raw_clock_reads_on_macos() {
    use crate::Clock;
    let c = UnixClock::from_clockid(libc::CLOCK_UPTIME_RAW);
    let t1 = c.now().unwrap();
    let t2 = c.now().unwrap();
    assert!((t2.seconds, t2.nanos) >= (t1.seconds, t1.nanos));
}
```

- [ ] **Step 2: 失敗を確認**

Run: `cd /Users/naxdev/Desktop/Devs/Rust/statime && cargo test -p clock-steering`
Expected: `fd` フィールドのコンパイルエラー 2 件。

- [ ] **Step 3: 実装**

- `from_clockid` と各 `pub const` 内の `fd: None,` に `#[cfg(target_os = "linux")]` を付ける。
- `pub const CLOCK_TAI` 全体を `#[cfg(target_os = "linux")]` で囲む(macOS に TAI クロックは無い。偽装しない)。

- [ ] **Step 4: 通ることと Linux 非破壊を確認**

Run: `cargo test -p clock-steering` と Docker での `cargo build -p clock-steering`
Expected: 両方成功。

- [ ] **Step 5: コミット・push(サブモジュール→親)**

```bash
git -C clock-steering add -A && git -C clock-steering commit -m "feat: build UnixClock on macOS (gate fd and CLOCK_TAI to linux)" && git -C clock-steering push
git add clock-steering && git commit -m "chore: bump clock-steering (macOS build)" && git push
```

---

### Task 3: timestamped-socket の macOS 対応(インターフェース固定 UDP とマルチキャスト)

**Files:**
- Create: `timestamped-socket/src/networkaddress/macos.rs`
- Create: `timestamped-socket/src/socket/macos.rs` への追加(既存ファイル。`configure_timestamping` のみある)
- Modify: `timestamped-socket/src/raw_socket/macos.rs`(`so_timestamp` のみある)
- Modify: `timestamped-socket/src/networkaddress.rs:10-13`(`mod macos` と `pub use` の追加)
- Modify: `timestamped-socket/src/socket.rs:18,27`(macOS の `pub use self::macos::*`)

**Interfaces:**
- Produces(Linux 版と同名・同シグネチャ):
  - `pub fn open_interface_udp4(interface: InterfaceName, port: u16, timestamping: InterfaceTimestampMode, bind_phc: Option<u32>) -> std::io::Result<Socket<SocketAddrV4, Open>>`
  - `pub fn open_interface_udp6(...)-> std::io::Result<Socket<SocketAddrV6, Open>>`(同様)
  - `impl MulticastJoinable for SocketAddrV4` / `SocketAddrV6`
  - `bind_phc` は無視。`timestamping` は `None` と `SoftwareRecv` のみ受け付け、他は `ErrorKind::Unsupported`。
- Consumes: `RawSocket::open/bind/reuse_addr/set_nonblocking/so_timestamp`(既存)、`interface::interfaces()`(macOS で動く)、`InterfaceName::get_index()`。

- [ ] **Step 1: 失敗するテスト(`socket.rs` の `mod tests`、`#[cfg(target_os = "macos")]`)**

```rust
#[tokio::test]
async fn open_udp4_on_loopback_gets_recv_timestamp() {
    let lo = InterfaceName::from_str("lo0").unwrap();
    let mut rx = open_interface_udp4(lo, 0, InterfaceTimestampMode::SoftwareRecv, None).unwrap();
    let port = match rx.local_addr().unwrap() { SocketAddr::V4(a) => a.port(), _ => unreachable!() };
    let mut tx = connect_address(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port), GeneralTimestampMode::None).unwrap();
    tx.send(&[1, 2, 3]).await.unwrap();
    let mut buf = [0u8; 8];
    let r = rx.recv(&mut buf).await.unwrap();
    assert_eq!(r.bytes_read, 3);
    assert!(r.timestamp.is_some(), "SO_TIMESTAMP must yield a software recv timestamp");
}

#[test]
fn open_udp4_on_missing_interface_names_it() {
    let bogus = InterfaceName::from_str("nope9").unwrap();
    let err = open_interface_udp4(bogus, 0, InterfaceTimestampMode::None, None).unwrap_err();
    assert!(err.to_string().contains("nope9"));
}

#[test]
fn software_all_is_unsupported_on_macos() {
    let lo = InterfaceName::from_str("lo0").unwrap();
    let err = open_interface_udp4(lo, 0, InterfaceTimestampMode::SoftwareAll, None).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
}
```
`Socket::local_addr()` が無ければ `getsockname` を包む `pub fn local_addr(&self) -> std::io::Result<A>` を `Socket<A, Open>` に追加する(Linux でも使える一般実装)。

- [ ] **Step 2: 失敗を確認**

Run: `cargo test -p timestamped-socket`
Expected: `open_interface_udp4` 未定義のコンパイルエラー。

- [ ] **Step 3: 実装**

`raw_socket/macos.rs` に追加(いずれも `setsockopt` の薄いラッパ。`InterfaceName → index` は `get_index()`、`InterfaceName → IPv4` は `interfaces()?[&name].ips()` の最初の V4。該当が無ければ `io::Error::new(NotFound, format!("interface {name} has no IPv4 address"))`):
- `bind_to_device(&self, InterfaceName)`: `IPPROTO_IP, libc::IP_BOUND_IF` に index。IPv6 ソケット用に `bind_to_device_v6` は `IPPROTO_IPV6, libc::IPV6_BOUND_IF`(いずれも libc 0.2.186 の apple モジュールに存在する)。
- `ip_multicast_if(&self, InterfaceName)`: `IP_MULTICAST_IF` に `in_addr`。
- `ipv6_multicast_if(&self, InterfaceName)`: `IPV6_MULTICAST_IF` に index。
- `ip_multicast_loop(bool)`, `ipv6_multicast_loop(bool)`, `ipv6_v6only(bool)`, `reuse_port()`。

`networkaddress/macos.rs`: `impl MulticastJoinable for SocketAddrV4`(`IP_ADD_MEMBERSHIP` / `IP_DROP_MEMBERSHIP` に `libc::ip_mreq { imr_multiaddr, imr_interface: <インターフェースの IPv4> }`)、`SocketAddrV6`(`IPV6_JOIN_GROUP` / `IPV6_LEAVE_GROUP` に `libc::ipv6_mreq { ipv6mr_multiaddr, ipv6mr_interface: index }`)。`SealedMC` も実装する。

`socket/macos.rs`: `open_interface_udp4/udp6` を Linux 版(`socket/linux.rs:227-280`)と同じ手順で、`driver_enable_timestamping` の呼び出しは無し、`send_counter` フィールドは無し。`configure_timestamping` は既存のまま(`SoftwareRecv` 以外は `Unsupported`)。

- [ ] **Step 4: 通ることと Linux 非破壊を確認**

Run: `cargo test -p timestamped-socket`、Docker で `cargo test -p timestamped-socket`
Expected: macOS で新テスト 3 件 PASS、Linux で既存テスト PASS。

- [ ] **Step 5: コミット・push(サブモジュール→親)**

```bash
git -C timestamped-socket add -A && git -C timestamped-socket commit -m "feat(macos): interface-bound UDP sockets, multicast join, SO_TIMESTAMP recv" && git -C timestamped-socket push
git add timestamped-socket && git commit -m "chore: bump timestamped-socket (macOS sockets)" && git push
```

---

### Task 4: statime-linux のクロックと設定の macOS 対応

**Files:**
- Modify: `statime-linux/src/clock/mod.rs`
- Modify: `statime-linux/src/config/mod.rs`(`SystemClockType` 19-45 行、`Config` に検証メソッド追加)
- Modify: `statime-linux/src/main.rs`(372-382 行、451-460 行、`CLOCK_TAI` 参照箇所)

**Interfaces:**
- Produces:
  - `SystemClockType::UptimeRaw`(設定文字列 `"UPTIME_RAW"` / `"uptime_raw"`)。
  - `LinuxClock::system_timestamp_clock() -> Self`: Linux では `CLOCK_TAI`(`is_tai = true`)、macOS では `from_clockid(libc::CLOCK_REALTIME)`。`main.rs` の `LinuxClock::CLOCK_TAI` 参照はすべてこれに置き換える。
  - `LinuxClock::guard_monotonic_clock() -> Self`: Linux は `CLOCK_MONOTONIC_COARSE`、macOS は `CLOCK_MONOTONIC`。
  - `Config::validate_platform(&self) -> Result<(), ConfigError>`: macOS で `virtual_system_clock == false`、`virtual_system_clock_base` が `UptimeRaw` 以外、`network_mode == Ethernet`、`hardware_clock` が `Required`/`Specific` のいずれかなら `ConfigError::Platform(String)`。Linux で `UptimeRaw` なら同じくエラー。
  - `clock::base_clockid(SystemClockType) -> libc::clockid_t`(main.rs の `match` を移動、`UptimeRaw => libc::CLOCK_UPTIME_RAW`)。
- Linux 限定に gate するもの: `LinuxClock::CLOCK_TAI`、`open`、`open_idx`、`system_offset`、`is_tai` を使う分岐。macOS では `get_tai_offset` は `Ok(0)`、`set_properties` は `Ok(())`(no-op)。

- [ ] **Step 1: 失敗するテスト(`config/mod.rs` の `mod tests`)**

```rust
#[test]
fn uptime_raw_parses() {
    let cfg: Config = toml::from_str(r#"
        virtual-system-clock = true
        virtual-system-clock-base = "uptime_raw"
        [[port]]
        interface = "en0"
    "#).unwrap();
    assert_eq!(cfg.virtual_system_clock_base, SystemClockType::UptimeRaw);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_requires_virtual_system_clock() {
    let cfg: Config = toml::from_str(r#"
        [[port]]
        interface = "en0"
    "#).unwrap();
    assert!(matches!(cfg.validate_platform(), Err(ConfigError::Platform(_))));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_rejects_ethernet_and_required_hw_clock() {
    for port in [r#"network-mode = "ethernet""#, r#"hardware-clock = "required""#] {
        let cfg: Config = toml::from_str(&format!(r#"
            virtual-system-clock = true
            virtual-system-clock-base = "uptime_raw"
            [[port]]
            interface = "en0"
            {port}
        "#)).unwrap();
        assert!(matches!(cfg.validate_platform(), Err(ConfigError::Platform(_))), "{port}");
    }
}
```
`clock/mod.rs` の `mod tests`(新規、`#[cfg(target_os = "macos")]`):
```rust
#[test]
fn macos_tai_offset_is_zero_and_set_properties_is_noop() {
    let mut c = LinuxClock::system_timestamp_clock();
    assert_eq!(c.get_tai_offset().unwrap(), 0);
    let props = statime::config::TimePropertiesDS::new_arbitrary_time(false, false, statime::config::TimeSource::InternalOscillator);
    assert!(statime::Clock::set_properties(&mut c, &props).is_ok());
}
```

- [ ] **Step 2: 失敗を確認**

Run: `cargo test -p statime-linux`
Expected: コンパイルエラー(`UptimeRaw`、`validate_platform`、`system_timestamp_clock` 未定義、既存 5 エラー)。

- [ ] **Step 3: 実装**

- `config/mod.rs`: `SystemClockType::UptimeRaw` と `"UPTIME_RAW"` の分岐、`ConfigError::Platform(String)`(`Display` も)、`validate_platform` を `from_file` の末尾で呼び、`Err` は従来の `ConfigError` 経路で終了させる。
- `clock/mod.rs`: `is_tai` フィールドと `CLOCK_TAI` / `open` / `open_idx` / `system_offset` を `#[cfg(target_os = "linux")]`。`system_timestamp_clock()`、`guard_monotonic_clock()`、`base_clockid()` を追加。macOS 用 `get_tai_offset` と `set_properties`。`set_frequency` / `step_clock` の `is_tai` 分岐は Linux のみ(macOS では `self.clock` に直接委譲。仮想クロック構成ではこの経路は呼ばれないが、コンパイルは通す)。
- `main.rs`: `LinuxClock::CLOCK_TAI` → `LinuxClock::system_timestamp_clock()`、`LinuxClock::from_clockid(libc::CLOCK_MONOTONIC_COARSE)` → `LinuxClock::guard_monotonic_clock()`、`match config.virtual_system_clock_base {...}` → `clock::base_clockid(...)`。`else { SystemClock::Linux(...) }`(非仮想モード)は `#[cfg(target_os = "linux")]`、macOS では `validate_platform` が先に弾く。
- `ClockExportGuard` の有効条件(`base == TAI && usrvclock_export` のときだけ無効)は変更しない(Review Focus 5)。

- [ ] **Step 4: ここまでで残るエラーを確認**

Run: `cargo build -p statime-linux 2>&1 | grep -E "^error" `
Expected: `socket.rs` の import エラー(`EthernetAddress`, `MacAddress`, `open_interface_ethernet`)と `add_hw_clock` 関連のみ(Task 5 で解消)。テストは Task 5 の後で実行する。

- [ ] **Step 5: コミット**

```bash
git add statime-linux/src/clock/mod.rs statime-linux/src/config/mod.rs statime-linux/src/main.rs
git commit -m "feat(macos): uptime_raw virtual clock base, platform config validation"
```

---

### Task 5: statime-linux のソケットと送信タイムスタンプの macOS 対応

**Files:**
- Modify: `statime-linux/src/socket.rs`(Ethernet 関連を Linux 限定に)
- Modify: `statime-linux/src/main.rs`(`add_hw_clock` 496-515 行、`add_sw_clock` 517-524 行、`NetworkMode::Ethernet` 分岐 596-612 行、`handle_actions` 983-1005 行)

**Interfaces:**
- Consumes: Task 3 の `open_interface_udp4/udp6`(macOS)、Task 4 の `validate_platform`。
- Produces: macOS ビルド可能な `statime` バイナリ。macOS のソフトウェア経路は `InterfaceTimestampMode::SoftwareRecv`。送信タイムスタンプ欠落時は `clock.now()` を使う。

- [ ] **Step 1: 失敗するテスト(`main.rs` 内は難しいので、送信タイムスタンプ代替を純関数に切り出してテストする)**

`statime-linux/src/lib.rs` に `pub mod sendts;` を追加し、`statime-linux/src/sendts.rs`:
```rust
use statime::time::Time;
/// Software send timestamp fallback: when the socket cannot report when the packet
/// left, the best estimate is the clock read immediately after `send_to` returned.
pub fn resolve_send_timestamp(from_socket: Option<Time>, read_after_send: Time) -> Time {
    from_socket.unwrap_or(read_after_send)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prefers_socket_timestamp() {
        let s = Time::from_nanos(100); let a = Time::from_nanos(200);
        assert_eq!(resolve_send_timestamp(Some(s), a), s);
    }
    #[test]
    fn falls_back_to_clock_read() {
        let a = Time::from_nanos(200);
        assert_eq!(resolve_send_timestamp(None, a), a);
    }
}
```

- [ ] **Step 2: 失敗を確認**

Run: `cargo test -p statime-linux sendts`
Expected: モジュール未定義でコンパイルエラー(加えて socket.rs のエラー)。

- [ ] **Step 3: 実装**

- `socket.rs`: `EthernetAddress`/`MacAddress`/`open_interface_ethernet` の import、`impl PtpTargetAddress for EthernetAddress`、`open_ethernet_socket` を `#[cfg(target_os = "linux")]`。ファイル冒頭の doc コメントを "Event and General sockets for unix systems" に。
- `main.rs`:
  - `use statime_linux::socket::open_ethernet_socket` と `use timestamped_socket::networkaddress::EthernetAddress` を Linux 限定 import に。`ethernet_port_task`、`handle_actions_ethernet`、`NetworkMode::Ethernet` 分岐を `#[cfg(target_os = "linux")]`(macOS では `validate_platform` が弾くので `unreachable!("rejected by validate_platform")` を置く)。
  - `add_hw_clock` クロージャと `HardwareClock::Required | Specific` 分岐を Linux 限定に(macOS 側は `unreachable!`)。`HardwareClock::Auto` は `lookup_phc()` が macOS で常に `None` を返すので既存コードのまま sw 経路に落ちる。
  - `add_sw_clock` の `InterfaceTimestampMode::SoftwareAll` を `if cfg!(target_os = "macos") { SoftwareRecv } else { SoftwareAll }`。
  - `handle_actions` の `PortAction::SendEvent`: `send_to` の直後に `let after = clock.now();` を取り、`pending_timestamp = Some((context, resolve_send_timestamp(time.map(|t| clock.port_timestamp_to_time(t)), after)))`。`None` だった場合のログは `log::error!("Missing send timestamp")` から `log::trace!("No socket send timestamp, using clock read after send")` に変える(macOS では毎回起きる)。Linux の動作は `Some` なら従来どおり。

- [ ] **Step 4: ビルドとテスト、Linux 非破壊を確認**

Run: `cargo test -p statime-linux` と Docker で `cargo test -p statime-linux`
Expected: macOS で全 PASS(Task 4 のテスト含む)、Linux で既存 2 テスト + 新規 PASS。

- [ ] **Step 5: コミット・push**

```bash
git add statime-linux
git commit -m "feat(macos): software-recv timestamping, send timestamp fallback, gate ethernet/PHC to linux"
git push
```

---

### Task 6: macOS 設定ファイルと実機検証(M1 の完了条件・判断点)

**Files:**
- Create: `statime/inferno-ptpv1-macos.toml`
- Create: `statime/docs/macos.md`(実行手順と測定結果の記録)

**Interfaces:**
- Consumes: inferno 側は Plan M0 完了済み(`cargo build -p inferno2pipe` が Mac で通る)。
- Produces: 設計 §6 M1 の完了判定に使う測定記録。

- [ ] **Step 1: 設定ファイルを書く**

`inferno-ptpv1.toml` を元に次を変える: `virtual-system-clock-base = "uptime_raw"`、`interface = "en0"`(実機のインターフェース名は `networksetup -listallhardwareports` で確認して差し替える)、`hardware-clock = "none"`、`loglevel = "debug"`。`protocol-version = "PTPv1"`、`usrvclock-export = true` はそのまま。

- [ ] **Step 2: 起動確認**

事前に同じ Mac の DVS を停止する(Dante Virtual Soundcard アプリで Stop)。
Run(別ターミナルで、sudo が必要): `sudo target/debug/statime -c inferno-ptpv1-macos.toml`
Expected: ログに PTPv1 マスターの検出と、オフセット測定行が周期的に出る。`ls -la /tmp/ptp-usrvclock` がソケットを示し、パーミッションが全ユーザー書き込み可。

- [ ] **Step 3: Inferno 側で受信確認**

Run(inferno リポジトリで): `RUST_LOG=debug ./target/debug/inferno2pipe -c 2 -o /dev/null 2>&1 | grep -i -m3 "clock"`
Expected: `clock path` ログの後、クロック未接続の警告が消え、オーバーレイ受信を示すログが出る。

- [ ] **Step 4: 1 時間録音と精度の実測**

Dante Controller で Dante 機器の出力 2ch を `Inferno2pipe` の RX 1-2 に接続し、
```bash
./target/debug/inferno2pipe -c 2 -o /tmp/rec.raw 2> /tmp/i2p.log &
sleep 3600; kill %1
ls -l /tmp/rec.raw; grep -c -i "hole\|xrun\|insane\|discontinuity" /tmp/i2p.log
```
Expected: ファイル長が `3600 * 48000 * 2ch * 4byte = 1,382,400,000` バイトの ±0.1% 以内。欠落系ログが 0 件(初回接続時の数件は許容)。Statime 側ログからオフセットの標準偏差を求め、1 ms 未満であること。`GUARD` の警告回数も記録する。

- [ ] **Step 5: 結果を文書化してコミット**

`docs/macos.md` に: ビルド手順(Xcode SDK の注意含む)、設定、実測値(ファイル長、欠落ログ件数、オフセット統計、GUARD 回数)、未解決事項。
```bash
git add inferno-ptpv1-macos.toml docs/macos.md
git commit -m "docs(macos): PTPv1 follower config and measurement results"
git push
```
判断点: Step 4 の期待値を満たさない場合はここで止め、設計書 §6 の見直しへ戻る。満たした場合のみ HAL プラグイン(M2 以降)の計画作成に進む。
