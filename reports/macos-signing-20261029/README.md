# macOS 维护版固定签名验证

目标发行版本为 `1.1.4+up.1.4.3`，数字版本 `1.1.4`，构建号 `6`。
本次只更改构建、签名、发行校验和文档，不更改应用的权限检测、截图、贴图、OCR 运行逻辑或用户配置。

## 身份策略

- Bundle ID：`io.github.trrywui.snowshot`。
- 证书：`Snow Shot Maintained (trrywui)`。
- 公开证书 SHA-1：`E2AC5BEA62E3D6643F4DC4ADB155F3BA31D9860E`。
- 指定要求：`identifier "io.github.trrywui.snowshot" and certificate leaf = H"e2ac5bea62e3d6643f4dc4adb155f3ba31d9860e"`。
- 私钥保存在登录钥匙串，代码签名信任仅设在当前用户范围内；构建、部署和打包不自动创建或更换身份。
- 旧 ad-hoc 包不满足新策略；使用该证书重签的工作区旧版基线包满足新策略。

固定身份针对的是后续发行包的签名连续性。首次从 ad-hoc 切换仍可能需要重新授权，不能把本次打包变更解释为当前已安装应用的授权已经自动修复。

## 相关回归

只运行相关脚本测试，共 31 项测试方法通过：

```sh
python3 scripts/test-macos14-signing.py
python3 scripts/test-macos14-personal.py
PATH="$PWD/.tools/macos14-personal/host/venv/bin:$PATH" python3 scripts/test-macos-build-support.py MacOSSigningDeployment MacOSBundleMetadata.test_native_deployment_fixture_has_no_mcp_bridge_requirement
```

分别为固定身份校验 12 项、维护构建流程 4 项、签名部署 14 项、原生部署 fixture 1 项。
覆盖原私钥缺失、证书或指定要求变化、旧缓存使用其他身份、最终校验失败、ad-hoc 回退拒绝以及首次初始化中断后的恢复边界。
Python、Shell、JSON 语法和 `git diff --check` 通过。未运行全量 CTest、Rust 测试或性能基准；应用运行逻辑及低占用策略没有改动。

## 本机应用边界

维护者最初同意本机覆盖验收，随后明确要求不要替换本机应用。此前已启动的基线安装已立即撤销，原应用原样恢复，恢复后主程序 SHA256 为
`1b43252c227cd6fa6951c7f50d4b2f9aef23f6565704df9dd60d0fbd28a1c1b0`，与保存的原件相同；原 ad-hoc 指定要求仍为
`cdhash H"0708fecd463a8235001d73f669fd2ef9ce6e79a7"`。

测试副本没有启动，屏幕录制和辅助功能授权没有改动。后续所有验证只针对工作区中的发行材料，不再替换 `/Applications/Snow Shot.app`。

实际覆盖升级后的权限继承、真实截图和辅助功能访问留待维护者手动验收。两个不同版本满足相同签名要求，只证明签名兼容性，不能替代这项实机验收。

## 手动验收

1. 退出当前 Snow Shot，手动安装固定签名版到 `/Applications/Snow Shot.app`。
2. 首次迁移如遇旧授权失配，在系统设置删除旧的屏幕录制和辅助功能条目，为该应用重新授权，再重启应用。
3. 确认实际截图、贴图及需要辅助功能的操作可用，退出应用。
4. 用同一证书签署的另一实际构建覆盖相同安装位置，重启后再次检查上述操作；期间不更改权限条目。
5. 记录系统版本、两个构建号和结果。实际继承失败时应保留具体错误再排查，不能仅凭系统设置开关认定通过。

## 初步构建审计结果

Release 构建、安装到工作区 staging、嵌套签名和 OCR 资源验证、Mach-O 系统依赖及 macOS 14.4 下限审计、DMG 完整性验证、只读挂载后的签名复验与 SHA256 校验均通过。
交付配置关闭所有测试和 benchmark，保持 x86_64、Release、大小优化与原有静态 Qt 配置。本次未启动任何应用副本。

工作区旧版基线 `1.1.3` / build `5` 的 CDHash 为 `526fdf072a5da9bba029e5a23f188c7d14a39938`；新版 `1.1.4` / build `6` 为 `4627c9de278431f95a3e1cb8a585b5df2bcb97b2`。
两个版本的主程序 SHA256 不同，固定证书和指定要求相同，并分别使用对方的指定要求完成 `codesign --verify --deep --strict -R` 检查。机器可读证据见 [signing-continuity.json](signing-continuity.json)。

以上证据来自提交源码前的初步构建，用于验证签名策略。最终发行包在源码冻结后重建，使内置诊断 Git 标识与源码提交一致，并重新完成同样的签名、资源、DMG 和校验和审计；最终文件大小和 SHA256 以 Release 的 `.sha256` 附件为准，不能拿初步构建的 CDHash 代替最终产物。
Release 附件仅包含 DMG 及其 `.sha256`，不上传依赖源码或任何私钥材料。

签名连续性审计已通过，实际权限继承仍待手动验收；正式发布时间须结合这一验收状态由维护者决定。
