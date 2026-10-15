# Snow Shot Intel macOS 维护分支

正式名称为 **Snow Shot**，版本 **1.1.0+up.1.3.0**，Bundle ID 为 `io.github.trrywui.snowshot`。
最低系统为 macOS 14.4，架构为 x86_64。由 trrywui 维护，源自 mg-chao/snow-apps 的 `8982d6660c03`。

## 构建隔离

所有命令在仓库根目录执行。沿用 `.tools/macos14-personal/` 和既有内部构建开关名称，
以复用已验证的工具链和缓存；内部名称不作为应用名称、版本后缀或 Bundle ID。
官方构建配置仍要求 macOS 15，此兼容配置默认不开启。

| 目录 | 用途 |
| --- | --- |
| `host`、`cargo`、`rustup` | 私有工具和 Rust 工具链 |
| `vcpkg`、`installed`、`qt-deps`、`qt` | 原生依赖和静态 Qt |
| `downloads`、`cache` | 源码下载和可复用缓存 |
| `build`、`performance`、`qt-build`、`rust-tests`、`native-tests` | 构建和测试中间文件 |
| `logs` | 构建、验证、性能采样记录 |
| `artifacts/<完整版本号>` | 本次发布的应用、DMG、校验和源码材料 |
| `user-data` | 仅通过开发脚本运行时使用的隔离测试数据 |

环境变量只影响脚本子进程，不修改全局 Rust 默认版本、Homebrew、shell 配置或系统证书。
ad-hoc 签名不创建钥匙串证书。默认并行度为 2，可用 `SNOW_PERSONAL_JOBS=1` 降低负载。
Release 的首次 LTO 链接可能持续数十分钟并使用数 GB 内存，这不是应用运行占用。
ThinLTO 链接缓存位于 `cache/thinlto`，供重复链接复用，不进入安装包。

## 构建和打包

使用现有 Xcode、Python 3、Git 和 rustup，其他工具由脚本准备在隔离目录。
已验证工具链为 Xcode 16.2 / SDK 15.2、Rust 1.97.1、Qt 6.12.0、CMake 4.4.4。
Xcode 16.2 的 C++20 线程取消使用其自带实验库选项，不替换全局 Xcode。

```sh
bash scripts/macos14-personal.sh all
bash scripts/macos14-personal.sh package
```

可单独重试 `bootstrap`、`qt-dependencies`、`qt`、`dependencies`、`configure`、`build`。
`macos14-personal-jpeg` feature 使 TurboJPEG 与静态 Qt 的 JPEG v8 ABI 一致，不应删除。

```text
.tools/macos14-personal/artifacts/1.1.0+up.1.3.0/Snow Shot.app
.tools/macos14-personal/artifacts/1.1.0+up.1.3.0/snow-shot-1.1.0+up.1.3.0-intel.dmg
.tools/macos14-personal/artifacts/1.1.0+up.1.3.0/snow-shot-1.1.0+up.1.3.0-intel.dmg.sha256
```

打包保留 OCR 模型、许可证和第三方声明，并验证签名、Mach-O 系统依赖及最低版本。
不覆盖已有同版本产物；重打包前先移动对应版本目录保存回退材料。

版本唯一配置为 `snow_shot/packaging/macos/maintained-release.json`。
应用和 Git 标签使用完整 SemVer；macOS 数字版本为 `1.1.0`，内部构建号为 `2`。
后续发布递增维护版本及构建号，上游版本需与根构建声明一致。

## 正式运行与开发测试

用户从 GitHub Release 手动下载安装；从旧个人兼容版首次切换时自行处理旧版。
正式维护版之间升级沿用配置和历史，正常退出后替换应用，无需清理数据。
双击应用使用 macOS 标准数据目录，不自动迁移原个人兼容版数据。
构建、清理和打包均不修改 `/Applications` 或已安装应用的数据和权限。
新配置恢复上游开机启动默认值，用户可自行关闭。

开发时可隔离运行：

```sh
bash scripts/macos14-personal.sh run --show-main-window
```

该命令在子进程设置 `SNOW_SHOT_PERSONAL_DATA_DIR`，配置、历史、缓存和诊断留在 `user-data`。
手动保存和导出由用户选择目录。清理不重置系统隐私权限，也不修改 Snipaste。
开发隔离目录不是正式安装的数据目录。

## 针对性验证

```sh
python3 scripts/test-macos14-personal.py
bash scripts/macos14-personal.sh native-tests
bash scripts/macos14-personal.sh app-tests
```

`native-tests` 检查维护版身份、更新请求隔离和上游更新逻辑。
`app-tests` 只运行设置/数据隔离、图像编码、配置备份恢复、登录项和关于界面相关测试，之后恢复交付配置。
测试不安装应用，不注册真实登录项，不修改已安装应用数据，不运行全量测试。

涉及 Rust API 判断时仅选择相关测试：

```sh
bash scripts/macos14-personal.sh test availability::
bash scripts/macos14-personal.sh test capabilities::
```

翻译使用 `translations` 模式提取，再补齐全部英文、简体中文、繁体中文目录。
`configure-tests` 只开启测试目标供 `build-target` 按名称构建，不运行全量测试。
`probe` 只验证 Qt / AppKit 初始化，不能代替截图、贴图、快捷键和睡眠恢复等交互验收。
基准测试只使用 `performance` 配置和相关目标。

## 内存和 CPU 采样

```sh
# 将 12345 换成实际 PID，可追加辅助进程 PID。
bash scripts/macos14-personal.sh measure 60 12345   > .tools/macos14-personal/logs/snow-idle.csv
```

每秒读取 physical footprint、resident memory、CPU 时间差和唤醒计数，不操作进程，不链接进应用。
CPU 100% 表示占满一个核心，计算均值时排除每个进程第一条记录。
进程退出或 PID 复用时直接报错；工具不会自动发现新辅助进程，也可能漏掉短暂峰值。
Resident memory 包含共享页，不能简单相加作为独占内存。
比较时对齐截图内容、像素尺寸、贴图数量、自动识别、历史策略和等待时间。
目前没有完成与 Snipaste 的公平性能对比，不宣称固定节省比例。

## 清理和发布

```sh
# 清理中间文件，保留工具、缓存、日志、产物和测试数据。
bash scripts/macos14-personal.sh clean
# 显式删除整个隔离目录，包括工具、缓存、产物和测试数据。
bash scripts/macos14-personal.sh clean --all
```

清理需要专用标记，拒绝危险符号链接；先退出使用这些目录的测试进程。
源码、Git 分支、已安装应用及其标准目录数据不会被删除。发布后不自动清理缓存。

本机构建并手动发布 GitHub Release，提供 DMG、SHA256、说明和对应源码材料。
使用 `bash scripts/macos14-personal.sh sources` 在隔离环境中收集并打包依赖源码，
按实际依赖库存中的校验和验证；同版本源码包已存在时拒绝覆盖。
源码材料包括 Qt 原始源码包、Rust registry 与锁定 Git crate 源码、原生依赖源码、vcpkg 配方及其本地补丁和来源清单。
本项目源码以发布标签的 Git 源码包提供；组合使用这两个源码包及构建说明。
上游发布流程位于 `.github/upstream-workflows/`，不会自动执行。
同步上游后重新检查 API 判断、JPEG ABI、依赖最低版本、Bundle ID、更新策略及相关功能。
