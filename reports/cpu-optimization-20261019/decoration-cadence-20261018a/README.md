# 30 Hz + Decoration Cache Experiment

Base: b7c03b439 (only presentation cadence). Branch: codex/screenshot-30hz-decoration-cache.
Only magnifier static decoration caching is added. Original opacity effect and capture
pixel format are retained. No paint logging, thread-pool changes, or backdrop refactoring.

The cached image is keyed by logical size, device pixel ratio, and theme background.
Tests cover reuse during pointer motion and new captures, coordinate and color updates,
theme invalidation, resizing, transparent margins, and original opacity behavior.
Offscreen testing also runs at scale factor 2; native Cocoa lifecycle is checked.

Manual measurement order: cache30 then full30. Both use 30 Hz, visible magnifier,
default GUI thread pool, identical isolated settings, and no paint or stack sampling.
CPU uses approximately one-second process CPU-time deltas; 100% is one logical core.
Pointer positions/buttons are recorded only while the test capture overlay exists.
Manual paths and areas vary; measured differences are not exact causal effects.

## 构建与相关验证

构建、签名、格式和差异检查通过。相关 offscreen 测试在默认缩放和
QT_SCALE_FACTOR=2 下均通过，Cocoa 原生缓存与窗口生命周期回归通过，
共三轮相关检查。未跑全套或 benchmark target。CPU 采样时编译已结束。
候选提交 `145459400`，SHA256 为 metadata.json 中记录的
`0b6087f20c44843dac4269a60790d5b75e9a464e7a4710ee7f8d76f7a1ad442c`。

## 本轮实测

顺序为缓存候选 cache30 → 完整优化版 full30。两轮均记录到一次完整截图
开始与结束，持续35.98 / 33.88秒；用户回复“两轮完成”，没有单独描述
放大镜外观、颜色、跟手或复制结果，不将截图窗口结束视作已验证复制成功。
两版相关配置完全一致，二进制SHA与归档记录一致，日志无拒绝权限、fatal
或paint采样输出。两版均正常放大镜、30 Hz、默认Qt GUI绘图线程池。

| 约一秒有效区间中位数 | 30 Hz＋装饰缓存 | 完整优化版 |
| --- | ---: | ---: |
| 移动：应用CPU | 23.58% | 23.55% |
| 框选：应用CPU | 31.32% | 28.53% |
| 静止：应用CPU | 0% | 0% |
| 有效移动/框选/静止区间 | 11/12/9 | 11/9/9 |
| 移动：指针路程（逻辑px/区间） | 757.89 | 484.77 |
| 框选：指针路程（逻辑px/区间） | 425.46 | 330.52 |
| 估算选区面积中位数（逻辑px） | 502551 | 381478 |
| 移动：WindowServer CPU | 42.08% | 42.06% |
| 框选：WindowServer CPU | 40.04% | 41.18% |

候选移动CPU比完整高0.025个百分点，框选高2.79个百分点。候选移动路程
约大56%，框选约大29%，估算面积约大32%。移动CPU有效区间范围为候选
4.89–27.44%、完整11.74–26.61%；框选分别29.46–36.11%、22.52–34.24%。
整轮RSS峰值206.03 / 205.56 MiB，包含多个阶段，不单独归因缓存内存。

## 解释与建议

本轮缓存候选接近完整优化版，值得作为最小方案优先保留。候选并非靠更慢
移动或更小选区得到较低CPU，但手动轨迹和面积未锁定，也未在本轮重测
仅30 Hz起点，因此不能称已证明性能等价或给出缓存独立贡献的精确百分比。

参考此前不同轮次：仅30 Hz为38.49/44.29%，30 Hz＋BGRA为38.29/43.70%，
30 Hz＋原生透明度为36.92/45.22%，均按移动/框选列出。这些是跨轮结果，
操作幅度不同，只用于观察趋势，不是统一受控单变量实验。

当前建议把“30 Hz＋装饰缓存”作为首选精简候选，暂不带入BGRA、原生
透明度和诊断日志；若继续确认，复用现有二进制复核候选即可，无需扩展
源码或重新构建。仍保留完整优化分支及各候选分支以便回退。

候选新增四个文件（含测试CMake定义）；连同起点限帧总六个文件，只有
两个.cpp运行逻辑文件和一个头文件，其余为测试或构建定义。源码工作区
干净。测试App及指针探针均退出，正式安装版PID550保留，未替换安装版。

## 缓存候选再次复测

用户要求复测缓存候选，复用同一SHA二进制，配置及30 Hz限制与上一轮一致，
未改源码、未编译。cache30-recheck1记录到一次完整截图，持续32.78秒。
用户明确确认放大镜阴影、颜色、跟手和复制均正常，无异常。

| 应用CPU有效区间中位数 | 缓存候选上一轮 | 缓存候选复测 | 完整版上一轮 |
| --- | ---: | ---: | ---: |
| 移动 | 23.58% | 24.58% | 23.55% |
| 框选 | 31.32% | 34.77% | 28.53% |
| 静止 | 0% | 0% | 0% |

复测有效移动/框选/静止区间为10/10/8。移动CPU范围6.89–26.45%，
框选25.51–38.20%。移动路程中位657.15、框选395.76逻辑px/区间，
估算选区面积中位447995逻辑px。相比自己上一轮，路程和面积均更小，
而框选CPU更高，表明还有轨迹、区域覆盖或系统状态等未控制因素；
不把变化精确归因于单一原因。WindowServer移动42.13%、框选43.58%。
整轮RSS峰值234.00 MiB，包含非截图阶段，不能当作缓存单项内存增量。

候选复测移动比自己上轮高1.00个百分点，框选高3.45个百分点；相比上一轮
完整优化版，移动高1.02个百分点、框选高6.24个百分点。完整优化版其它
近期轮次框选约28.50–31.96%，缓存候选两轮为31.32/34.77%。因此移动
低占用趋势可重复，框选仍可能有差距，不能称为性能完全等价。

若优先减少长期维护和上游合并范围，仍建议保留“30 Hz＋装饰缓存”作为
精简候选，同时接受当前实测框选比完整版可能多约3–6个百分点的取舍。
不据此认定BGRA或原生透明度对所有场景都无价值，不直接替换正式版。
本轮仅增加归档数据，源码仍为145459400，工作区干净；测试及采样进程
已退出，正式版PID550保留。本轮不重复运行未变更源码的构建和单元测试。
