# M0: Inferno リポジトリの macOS ビルド対応 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `inferno_aoip` / `inferno2pipe` / `usrvclock-rs` が macOS でビルド・テストでき、HAL プラグインが必要とする 3 つの設定(macOS 用 RT スレッド、`STATE_DIR`、受信オーバーレイの検証)が入った状態にする。Linux ビルドは壊さない。

**Architecture:** 既存コードの Linux 依存箇所を `cfg(target_os)` で分岐させる最小修正。新クレートは作らない。作業はすべて開発者自身のフォーク(GitHub `Naxii-e`)で行い、usrvclock-rs サブモジュールの URL をフォークに向け直す。

**Tech Stack:** Rust(stable)、cargo、`libc` 0.2.186(mach API 含む)、`nix` 0.29、`thread-priority` 0.15、Docker(Linux 非破壊確認用)、`gh` CLI。

**Spec:** `docs/superpowers/specs/2026-11-20-macos-coreaudio-virtual-soundcard-design.md`(§3 制約、§5.1、§6 M0)

## Global Constraints

- Linux のビルド・テストを壊さない。macOS 固有コードは `#[cfg(target_os = "macos")]` の中だけに置く。
- 作業は Naxii-e のフォークで行う。上流 `teodly/inferno` と `gitlab.com/lumifaza/usrvclock-rs` には push しない。
- `alsa_pcm_inferno` は macOS ではビルドできない(`alsa-sys-all` が pkg-config の `alsa` を要求)。macOS では常に `-p` 指定でビルド・テストする。
- 既存の整形規約に合わせる(`cargo fmt` を通す。インデントはクレートごとの既存スタイルに従う)。
- コミットメッセージは英語、Conventional Commits 風(`feat:`, `fix:`, `test:`, `docs:`, `chore:`)。末尾に `Co-Authored-By: Claude Mythos 5.1 <noreply@anthropic.com>` を付ける。

## Review Focus

1. **PTP デーモンが Linux の `clock_id` を配信してきた場合**(Mac で Linux 用設定を流用した等): `usrvclock-rs/src/lib.rs:116` の `clock.now().unwrap()` がプロセスごと落とす。受信側は不正な `clock_id` のオーバーレイを捨ててエラーとして報告すべき。→ Task 2 のテスト `recv_rejects_invalid_clock_id`。
2. **`STATE_DIR` が書けない場所を指している場合**: 保存失敗はログに残して続行し、panic しない。→ Task 5 のテスト `unwritable_state_dir_does_not_panic`。
3. **RT スレッド設定が拒否される環境**(coreaudiod のサンドボックス): `set_current_thread_realtime` は `Err` を返し、呼び出し側(`flows_rx.rs:195`, `flows_tx.rs:275`)は結果を捨てて続行する。panic しない。→ Task 4 のテスト `realtime_failure_is_err_not_panic`。
4. **`CLOCK_PATH` のソケットが存在しない**(デーモン未起動): 既存動作(再試行し続ける)が macOS でも同じであること。→ Task 6 の検証手順で `inferno2pipe` を起動して確認。
5. **サブモジュール URL の変更で Linux ユーザーの `git clone --recursive` が壊れる**: フォーク先が公開リポジトリであること、`git submodule update --init --recursive` が新規クローンで通ること。→ Task 1 の検証手順。

---

### Task 1: フォーク作成とリモート/サブモジュールの付け替え

**Files:**
- Modify: `.gitmodules`(usrvclock-rs の `url` と `branch`)
- Add: `docs/superpowers/specs/2026-11-20-macos-coreaudio-virtual-soundcard-design.md`, `docs/superpowers/plans/*.md`(既に作業ツリーにある。最初のコミットに含める)

**Interfaces:**
- Produces: GitHub リポジトリ `Naxii-e/inferno`(ブランチ `macos`)、`Naxii-e/usrvclock-rs`(ブランチ `next-macos`)。以降の全タスクはこの 2 つに push する。

- [ ] **Step 1: inferno をフォークし、リモートを付け替える**

```bash
cd /Users/naxdev/Desktop/Devs/Rust/inferno
gh repo fork teodly/inferno --remote=true --remote-name=fork --clone=false
git remote rename origin upstream
git remote rename fork origin
git checkout -b macos dev
git remote -v
```
Expected: `origin` が `https://github.com/Naxii-e/inferno`、`upstream` が `teodly/inferno`。

- [ ] **Step 2: usrvclock-rs を GitHub にミラーし、ブランチを作る**

サブモジュールは GitLab 上流なので `gh repo fork` は使えない。サブモジュールの作業ツリーから新規リポジトリを作って push する。

```bash
cd /Users/naxdev/Desktop/Devs/Rust/inferno/usrvclock-rs
git stash          # スパイクで入れた lib.rs の 7 行変更を一時退避(Task 2 で正式に入れ直す)
git checkout -b next-macos
gh repo create Naxii-e/usrvclock-rs --public --source=. --remote=fork --push
git push fork next-macos
git stash pop
```
Expected: `https://github.com/Naxii-e/usrvclock-rs` にブランチ `next` 相当の履歴と `next-macos` が存在する。

- [ ] **Step 3: `.gitmodules` をフォークに向ける**

`.gitmodules` の `[submodule "usrvclock-rs"]` を次にする(他の 2 つのサブモジュールは変更しない):

```
[submodule "usrvclock-rs"]
	path = usrvclock-rs
	url = https://github.com/Naxii-e/usrvclock-rs
	branch = next-macos
```
その後 `git submodule sync usrvclock-rs`。

- [ ] **Step 4: コミットして push**

```bash
cd /Users/naxdev/Desktop/Devs/Rust/inferno
git add .gitmodules docs/superpowers
git commit -m "chore: point usrvclock-rs submodule at fork, add macOS design and plans"
git push -u origin macos
```

- [ ] **Step 5: 新規クローンで取得できることを検証(Review Focus 5)**

```bash
cd /private/tmp/claude-501/-Users-naxdev-Desktop-Devs-Rust-inferno/bd5ea4f2-d147-4aed-9c6e-e208c6462fcd/scratchpad
rm -rf clone-check && git clone --recursive -b macos https://github.com/Naxii-e/inferno clone-check 2>&1 | tail -3
```
Expected: 3 つのサブモジュールがすべて checkout される。

---

### Task 2: usrvclock-rs の非 Linux 対応

**Files:**
- Modify: `usrvclock-rs/src/lib.rs`(`freq_scale_including_hw` 付近 160-176 行、`safe_clock` テスト 699 行、`ClockOverlay::decode` 121-143 行、`OverlayReceiveError`)

**Interfaces:**
- Produces: `ClockOverlay::freq_scale_including_hw(&self) -> f64` が全 unix で存在。`OverlayReceiveError::InvalidClockId` 変種。`Client::recv` は不正な `clock_id` を `Err(InvalidClockId)` で返す。

- [ ] **Step 1: 失敗するテストを書く(`lib.rs` の `mod tests`)**

```rust
#[cfg(feature="unix")]
#[test]
fn freq_scale_including_hw_without_kernel_adjustment_equals_freq_scale() {
    let overlay = ClockOverlay {
        clock_id: nix::libc::CLOCK_MONOTONIC as i64,
        last_sync: 0, shift: 0, freq_scale: 1.5e-6,
    };
    // Linux では clock_adjtime の freq が 0 でなければ一致しないので非 Linux のみ検証
    #[cfg(not(target_os = "linux"))]
    assert_eq!(overlay.freq_scale_including_hw(), 1.5e-6);
}

#[cfg(feature="unix")]
#[test]
fn decode_rejects_invalid_clock_id() {
    let mut buff = vec![0u8; OVERLAY_SIZE_BYTES];
    buff[0] = b'V'; buff[1] = b'C';
    buff[2..4].copy_from_slice(&PROTOCOL_MAJOR_VERSION.to_ne_bytes());
    buff[6..8].copy_from_slice(&1u16.to_ne_bytes());
    buff[8..16].copy_from_slice(&999_999i64.to_ne_bytes()); // 存在しない clockid
    assert!(matches!(ClockOverlay::decode(&buff), Err(OverlayReceiveError::InvalidClockId)));
}
```
`safe_clock` テストの `nix::time::ClockId::CLOCK_MONOTONIC_RAW.as_raw()` は `nix::libc::CLOCK_MONOTONIC_RAW`(Linux と macOS の両方に存在)に置き換える。

- [ ] **Step 2: 失敗を確認**

Run: `cd /Users/naxdev/Desktop/Devs/Rust/inferno && cargo test -p usrvclock 2>&1 | tail -20`
Expected: macOS では `freq_scale_including_hw` が見つからないコンパイルエラー、または `InvalidClockId` 未定義。

- [ ] **Step 3: 実装**

- `#[cfg(not(target_os = "linux"))] pub fn freq_scale_including_hw(&self) -> f64 { self.freq_scale }`(Linux 版の直後。既存の Linux 版はそのまま)。
- `OverlayReceiveError` に `InvalidClockId` を追加。
- `ClockOverlay::decode` の末尾で、`feature="unix"` のとき `nix::time::ClockId::from_raw(clock_id as _).now()` が `Err` なら `Err(OverlayReceiveError::InvalidClockId)`。デコードは受信ごとに呼ばれるが `clock_gettime` 1 回の追加コストは許容する。

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test -p usrvclock`
Expected: 全テスト PASS(`safe_clock` を含む)。

- [ ] **Step 5: サブモジュール側でコミット・push し、親リポジトリのポインタを更新**

```bash
cd /Users/naxdev/Desktop/Devs/Rust/inferno/usrvclock-rs
git add src/lib.rs
git commit -m "feat: build on non-Linux unix, reject overlays with invalid clock_id"
git push fork next-macos
cd .. && git add usrvclock-rs && git commit -m "chore: bump usrvclock-rs (non-Linux support)" && git push
```

---

### Task 3: inferno_aoip のループバック結合テストを macOS で通す

**Files:**
- Modify: `inferno_aoip/tests/loopback_trx.rs:53`
- Modify: `inferno_aoip/Cargo.toml`(`[dev-dependencies]` に `libc = "0.2"`)

**Interfaces:**
- Consumes: Task 2 の usrvclock(不正 `clock_id` を拒否するため、`1i64` のままだと macOS では受信されず テストがタイムアウトする)。

- [ ] **Step 1: 現状の失敗を確認**

Run: `cargo test -p inferno_aoip --test loopback_trx 2>&1 | tail -5`
Expected: macOS で FAIL(クロックが来ない、またはパニック)。

- [ ] **Step 2: 修正**

`clock_id: 1i64, // CLOCK_MONOTONIC` を `clock_id: libc::CLOCK_MONOTONIC as i64,` に変える(Linux では 1、macOS では 6)。

- [ ] **Step 3: 通ることを確認**

Run: `cargo test -p inferno_aoip`
Expected: 単体 92 件 + `test_loopback_trx` PASS。

- [ ] **Step 4: コミット**

```bash
git add inferno_aoip/tests/loopback_trx.rs inferno_aoip/Cargo.toml Cargo.lock
git commit -m "test: use platform CLOCK_MONOTONIC id in loopback test"
```

---

### Task 4: macOS 用 `set_current_thread_realtime`

**Files:**
- Modify: `inferno_aoip/src/util/os.rs`
- Modify: `inferno_aoip/Cargo.toml`(`[target.'cfg(target_os = "macos")'.dependencies] libc = "0.2"`)

**Interfaces:**
- Produces: `pub fn set_current_thread_realtime(priority_hint: u8) -> Result<(), thread_priority::Error>` が macOS でも同じシグネチャで存在(既存の呼び出し元 `flows_rx.rs:195`, `flows_tx.rs:275` は変更しない)。

- [ ] **Step 1: 失敗するテストを書く(`os.rs` 末尾、`#[cfg(all(test, target_os = "macos"))]`)**

```rust
#[test]
fn realtime_succeeds_in_normal_process() {
    // 通常プロセスでは thread_policy_set(THREAD_TIME_CONSTRAINT_POLICY) は成功する(実機で確認済み)
    std::thread::spawn(|| set_current_thread_realtime(80)).join().unwrap().unwrap();
}

#[test]
fn realtime_failure_is_err_not_panic() {
    // 無効な mach ポート(0)に対する呼び出しは Err を返す経路を通ること
    assert!(set_thread_time_constraint(0, 80).is_err());
}
```

- [ ] **Step 2: 失敗を確認**

Run: `cargo test -p inferno_aoip os::`
Expected: `set_thread_time_constraint` 未定義でコンパイルエラー。

- [ ] **Step 3: 実装(macOS 分岐)**

- 既存の unix 実装の属性を `#[cfg(all(target_family = "unix", not(target_os = "macos")))]` に変更。
- 追加: `#[cfg(target_os = "macos")] pub fn set_current_thread_realtime(priority_hint: u8) -> Result<(), Error>` は `set_thread_time_constraint(unsafe { libc::pthread_mach_thread_np(libc::pthread_self()) }, priority_hint)` を呼ぶ。
- 追加: `#[cfg(target_os = "macos")] fn set_thread_time_constraint(thread: libc::mach_port_t, _priority_hint: u8) -> Result<(), Error>`。`libc::mach_timebase_info` で ns→tick 係数を取り、`libc::thread_time_constraint_policy { period: 1ms, computation: 200µs, constraint: 500µs, preemptible: 1 }` を tick に換算して `libc::thread_policy_set(thread, libc::THREAD_TIME_CONSTRAINT_POLICY as _, ptr, THREAD_TIME_CONSTRAINT_POLICY_COUNT)` に渡す。`KERN_SUCCESS` 以外は `Err(Error::OS(kr as i32))`。`THREAD_TIME_CONSTRAINT_POLICY_COUNT` は `size_of::<thread_time_constraint_policy>() / size_of::<libc::integer_t>()` で求める。
- 既存の `#[cfg(not(target_family = "unix"))]` 分岐はそのまま。

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test -p inferno_aoip os::` と `cargo build -p inferno2pipe`
Expected: 両テスト PASS、ビルド成功。

- [ ] **Step 5: コミット**

```bash
git add inferno_aoip/src/util/os.rs inferno_aoip/Cargo.toml Cargo.lock
git commit -m "feat(macos): realtime threads via thread_policy_set"
```

---

### Task 5: `STATE_DIR` 設定

**Files:**
- Modify: `inferno_aoip/src/device_server/settings.rs`(`Settings` 構造体 155-160 行、`Settings::new` 194-205 行)
- Modify: `inferno_aoip/src/state_storage.rs:21-28`
- Modify: `inferno_aoip/src/device_server/mod.rs:87`
- Modify: `inferno_aoip/src/device_info.rs:53-80`(テスト用コンストラクタ `dummy_device_info` は現在 `mod tests` の中にある。`impl DeviceInfo` の `#[cfg(test)] pub(crate) fn dummy_device_info(latency_ns: usize, sample_rate: u32) -> Self` に移し、既存テストは `DeviceInfo::dummy_device_info(...)` 呼びに変える)

**Interfaces:**
- Produces: `Settings.state_dir: Option<PathBuf>`(設定キー `STATE_DIR`、環境変数 `INFERNO_STATE_DIR`)。`StateStorage::new(self_info: &DeviceInfo, base_dir: Option<&Path>) -> Self`。`base_dir` が `None` なら従来どおり `platform_dirs` の state_dir。

- [ ] **Step 1: 失敗するテストを書く**

`state_storage.rs` の `mod tests`:
```rust
#[test]
fn explicit_base_dir_is_used() {
    let dir = std::env::temp_dir().join(format!("inferno-state-test-{}", std::process::id()));
    let info = DeviceInfo::dummy_device_info(0, 48000);
    let storage = StateStorage::new(&info, Some(dir.as_path()));
    assert!(storage.path_prefix.starts_with(dir.to_str().unwrap()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unwritable_state_dir_does_not_panic() {
    let info = DeviceInfo::dummy_device_info(0, 48000);
    let storage = StateStorage::new(&info, Some(std::path::Path::new("/nonexistent-root/inferno")));
    assert!(storage.save("x", &1u32).is_err());
}
```
`settings.rs` の `mod tests`(無ければ作る):
```rust
#[test]
fn state_dir_setting_is_parsed() {
    let mut cfg = BTreeMap::new();
    cfg.insert("STATE_DIR".to_string(), "/tmp/inferno-x".to_string());
    cfg.insert("BIND_IP".to_string(), "127.0.0.1".to_string());
    let s = Settings::new("T", "T", None, &cfg);
    assert_eq!(s.state_dir, Some(PathBuf::from("/tmp/inferno-x")));
}
```

- [ ] **Step 2: 失敗を確認**

Run: `cargo test -p inferno_aoip state_dir`
Expected: コンパイルエラー(フィールド・引数が無い)。

- [ ] **Step 3: 実装**

`Settings` に `pub state_dir: Option<PathBuf>` を追加し `config.get("STATE_DIR").map(PathBuf::from)` で埋める。`StateStorage::new` に `base_dir` 引数を追加し、`Some(dir)` なら `dir.join(hex::encode(factory_device_id))` を使う。`DeviceServer::start` の呼び出しを `StateStorage::new(&self_info, settings.state_dir.as_deref())` に変える。`create_dir_all` 失敗は既存どおり `log_and_forget`。

- [ ] **Step 4: 通ることを確認**

Run: `cargo test -p inferno_aoip`
Expected: 全 PASS。

- [ ] **Step 5: コミット**

```bash
git add inferno_aoip/src
git commit -m "feat: STATE_DIR setting to override state storage location"
```

---

### Task 6: macOS ビルド手順の文書化と Linux 非破壊確認

**Files:**
- Modify: `README.md`(「Quick start」の後に小節を追加)

- [ ] **Step 1: README に macOS 小節を追加**

見出し `## Building on macOS (experimental)`。内容は 3 点だけ: (1) 現状は `inferno2pipe` と `inferno_aoip` ライブラリのみビルド可、ALSA プラグインは対象外、(2) コマンド `cargo build -p inferno2pipe` と `cargo test -p inferno_aoip -p usrvclock -p searchfire`、(3) クロック同期デーモンの macOS 版は別途(`docs/superpowers/plans/2026-11-20-m1-statime-macos-daemon.md` を参照)。

- [ ] **Step 2: macOS で全体を検証**

Run:
```bash
cargo build -p inferno2pipe && cargo test -p inferno_aoip -p usrvclock -p searchfire 2>&1 | grep "test result"
./target/debug/inferno2pipe -c 2 -o /dev/null & sleep 5; kill %1
```
Expected: ビルド成功、全 `test result: ok`、`inferno2pipe` がクロック未接続の警告を出しつつ 5 秒間落ちずに動く(Review Focus 4)。

- [ ] **Step 3: Linux で壊れていないことを Docker で検証**

```bash
docker run --rm -v "$PWD":/build -w /build rust:1.92.0-slim-trixie sh -c \
  "apt-get update -qq && apt-get install -y -qq libasound2-dev pkg-config >/dev/null && cargo test --workspace 2>&1 | grep -E 'test result|error'"
```
Expected: `error` 行なし、全 `test result: ok`。Docker が無い場合はその旨を記録して人に確認を求める(勝手にスキップ扱いにしない)。

- [ ] **Step 4: コミットして push**

```bash
git add README.md
git commit -m "docs: macOS build notes"
git push
```
