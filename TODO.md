# TODO

## P68：英文主 README 与 GitHub 项目描述

1. [x] 将主 README 改为英文，并新增可互相跳转的中文 README。
2. [x] 将 GitHub 项目描述改为英文，并核对两种语言的项目范围一致。

## P62：Terrain 切片耗时优化

1. [x] 记录基线并定位热点。
2. [ ] 核对 C++ 实现并验证候选改动。
3. [ ] 复测耗时、峰值 RSS 和瓦片，运行全目标测试并提交。

## P63：升级 TIFF 读取依赖并复测 CI

1. [x] 升级并核对依赖解析。
2. [x] 验证兼容性并提交推送。
3. [x] 检查 CI benchmark 汇总。

P62 的内联候选在 12–6 级本地样本上中位耗时从 16.44 秒降至 15.94 秒。
为独立评估依赖升级，P63 提交未包含该候选。

## P64：升级其余直接依赖并复测 CI

1. [x] 查询版本并通过 Cargo CLI 升级。
2. [x] 验证锁文件和全目标测试并提交。
3. [x] 推送并比较 CI benchmark。

## P65：本地试验 Quick XML 0.42

1. [x] 建立本地分支和旧版基线。
2. [x] 升级依赖并适配 VRT 读取接口。
3. [x] 对照性能与瓦片内容，统计并提交改动。

## P66：扩大 Quick XML 本地样本

1. [x] 筛选较大可用的 VRT 瓦片样本。
2. [x] 交错复测并记录差分结果。

## P67：Terrain NoData 掩模处理

1. [x] 更新 Terrain 行为文档和本计划。
2. [x] 新增 NoData 单元与集成失败测试。
3. [x] 在 Terrain pooled average 路径携带当前采样层 NoData，并过滤样本。
4. [x] 验证 `/tmp` 最小 Float32 GeoTIFF、直接源与缓存源结果一致。
5. [x] 分阶段提交，并执行格式和全目标测试。

`cargo clippy --all-targets -- -D warnings` 因既有 `src/geotiff.rs`、
`src/sampling.rs` 和 `src/terrain.rs` 中的 10 个 lint 失败；相关文件未经授权未修改。
