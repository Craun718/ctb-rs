# oxiarc-lzw 解码性能源码分析

## 1. 范围与结论

本文分析当前 `Cargo.lock` 实际解析的 `oxiarc-lzw 0.4.0`，聚焦 GeoTIFF/TIFF
LZW 解码路径。分析只基于依赖源码和 P37 后已有的采样结果；不评估正确性差异，
也不在本阶段提出或实施依赖替换。

源码版本与路径：

- `oxiarc-lzw 0.4.0/src/lib.rs`
- `oxiarc-lzw 0.4.0/src/decoder.rs`
- `oxiarc-lzw 0.4.0/src/dictionary.rs`
- `oxiarc-lzw 0.4.0/src/bitstream_msb.rs`
- `oxigeo-geotiff 0.2.3/src/compression/mod.rs`
- `oxigeo-geotiff 0.2.3/src/cog/mod.rs`

核心结论如下：

1. `oxiarc-lzw` 的主要开销不在 TIFF 的 MSB-first 位流本身，而在解码器的数据
   结构和对象生命周期。字典把每个 code 对应的完整字节串保存为独立
   `Vec<u8>`，解码时继续克隆这些字节串。
2. one-shot API 为每个 TIFF strip/tile 新建 decoder。新建字典已经完整 reset
   一次，`decode()` 入口又 reset 一次；TIFF 流通常以 ClearCode 开始，因此还会
   执行协议要求的第三次 reset。reset 会丢弃并重建 256 个单字节 `Vec<u8>`，
   还会构建解码器从不使用的反向 `HashMap<Vec<u8>, u16>`。
3. 解码每个常规 code 通常至少创建两个堆分配字符串：当前 code 的字符串副本，
   以及新字典条目 `prev_string + first_byte` 的副本；随后 `extend_from_slice`
   还要把当前字符串复制进输出。字符串长度随 LZW 匹配增长，因此这不是固定
   小开销。
4. 解码输出先写入 owned `Vec<u8>`。初始 reservation 被 64 KiB 上限约束，这是
   面向不可信输入的合理保护；但对更大的 GeoTIFF block，会触发几何增长
   realloc。OxiGeo 随后把这个 owned buffer 再复制到调用方 `dst`，增加一次
   全 payload 复制。
5. `0.4.1` 的 `decoder.rs` 与 `0.4.0` 完全一致。仅升级到 0.4.1 不能解决本文
   列出的解码热区。

按影响排序，主因依次是：

1. 每个解码 code 的完整字符串克隆和字典条目存储；
2. one-shot decoder 的构造、重复 reset、条带结束 teardown；
3. 输出 `Vec` 的分配/增长以及 OxiGeo 的中间 buffer 复制；
4. MSB bit reader 的逐字节填充和分支。它有优化空间，但不是当前主要瓶颈。

## 2. 实际调用链

项目内 GeoTIFF block cache 最终进入 OxiGeo 的 tile 直读接口：

```text
ctb-rs GeoTIFF block cache
  -> oxigeo_geotiff::CogReader::read_tile_into
  -> oxigeo_geotiff::compression::decompress_into_partial
  -> oxigeo_geotiff::compression::decompress
  -> oxiarc_lzw::decompress_tiff
  -> oxiarc_lzw::decompress
  -> LzwDecoder::new
  -> LzwDecoder::decode
  -> LzwDictionary
```

关键源码行为：

- `lib.rs:106-108`：`decompress()` 每次调用创建新的 `LzwDecoder`。
- `lib.rs:159-160`：`decompress_tiff()` 只是转入同一个 one-shot `decompress()`。
- `decoder.rs:57-65`：`decode()` 入口 reset 字典并创建 owned 输出 `Vec`。
- `decoder.rs:113-147`：解码当前字符串、输出字符串并构造新字典条目。
- `compression/mod.rs:122-155`：OxiGeo 的 LZW 走通用 `_` 分支，先得到 owned
  `Vec<u8>`，再复制到调用方 `dst`。
- `cog/mod.rs:811-875`：`read_tile_into()` 对外提供 caller-owned destination；
  但在 LZW 场景下，这个“直读”只发生在依赖返回中间 owned buffer 之后。

因此，当前 profile 中的 LZW 热区包含三层成本：

1. `oxiarc-lzw` 内部的字典、分配和字符串复制；
2. 解码输出的 `Vec` 增长；
3. OxiGeo 从 owned `Vec` 到 `dst` 的边界复制。

## 3. one-shot 生命周期带来的重复初始化

### 3.1 每个 block 新建 decoder

`oxiarc_lzw::decompress()` 的实现是：

```rust
pub fn decompress(data: &[u8], expected_size: usize, config: LzwConfig) -> Result<Vec<u8>> {
    let mut decoder = LzwDecoder::new(config)?;
    decoder.decode(data, expected_size)
}
```

GeoTIFF 的每个 strip/tile 是独立 LZW 流。当前 API 每个 block 都执行一遍：

1. 分配 `LzwDictionary` 的外层 `table` 容量；
2. 初始化 256 个单字节条目；
3. 构建反向 `HashMap`；
4. 解码；
5. drop 字典、HashMap 和输出 buffer。

即使项目侧 native block cache 保证同一个源 block 只解码一次，上述成本仍会在
所有首次进入 cache 的 block 上发生。它不是缓存重复命中造成的。

### 3.2 构造后立即重复 reset

`LzwDictionary::new()` 在 `dictionary.rs:27-39` 构造对象后立即调用
`reset()`。`LzwDecoder::decode()` 在 `decoder.rs:57-60` 又无条件调用
`self.dict.reset()`。

对 one-shot API 来说，构造函数建好的初始字典马上被丢弃并重建一次。这个重建
没有任何语义收益。

TIFF LZW 流还通常以 ClearCode 开始。`decoder.rs:88-99` 读到 ClearCode 后再次
调用 `self.dict.reset()`。这次 reset 是接受 TIFF 流所必需的；问题在于 one-shot
路径中前两次初始化已经保证表处于初始状态，导致同一个 block 开始时实际执行：

```text
LzwDictionary::new -> reset
LzwDecoder::decode -> reset
stream ClearCode   -> reset
```

如果未来优化为复用 decoder，`decode()` 入口 reset 或流内 ClearCode 仍需保留
其中一种状态恢复机制；但当前“新建对象 + 入口 reset + 流内 ClearCode”的组合
在 one-shot 路径上明显多余。

## 4. 字典表示是核心热区

### 4.1 完整字节串和编码用反向表同时存在

`dictionary.rs:12-18` 的字典结构是：

```rust
pub struct LzwDictionary {
    table: Vec<Vec<u8>>,
    reverse: HashMap<Vec<u8>, u16>,
    config: LzwConfig,
    next_code: u16,
    current_bits: u8,
}
```

这带来三个问题。

第一，每个 code 都拥有完整字节串。解码器理论上只需要从 code 反推字节串；
常见高性能实现会保存 `prefix_code`、结尾字节和字符串长度，而不是为每个条目
保存独立增长的 `Vec<u8>`。

第二，`reverse` 只服务 `find_code()` 编码查找。解码路径不调用
`find_code()`，也不需要在 `HashMap` 中以字节串作为 key。但 `reset()` 仍然为
解码器构建并维护这个表。

第三，每个新条目都从旧字符串克隆出新的 owned `Vec<u8>`。表项数量上限虽然
受 LZW 12-bit 代码空间约束，但字符串本身可以增长；这带来大量小对象分配、
内存碎片和 drop 成本。

### 4.2 reset 本身是昂贵操作

`dictionary.rs:43-61` 的 reset 流程是：

1. `table.clear()`：drop 已存在的所有 owned `Vec<u8>`；
2. `reverse.clear()`：drop `HashMap<Vec<u8>, u16>` 的 key；
3. 循环 256 次，为每个 byte 新建 `Vec<u8>`；
4. clone 同一个字节串并插入反向 `HashMap`；
5. 再 push clear code 和 EOI 的两个空 `Vec<u8>`。

在解码过程中遇到 ClearCode 时，这个流程还会把已建立的字典条目逐个 drop，然后
重新执行 256 次单字节分配。相比之下，紧凑表结构的 reset 可以近似为恢复
`next_code`、`current_bits` 和固定长度数组中的初始 258 项，不需要重建 owned
字符串，也不需要触碰编码用反向表。

### 4.3 常规 code 触发多次复制

`decoder.rs:113-147` 的主循环行为如下。

已存在 code 的常见分支：

```rust
let string = self.dict.get_string(code)?.to_vec();
```

这里发生第一次堆分配和完整字符串复制。随后：

```rust
output.extend_from_slice(&string);
```

当前字符串第二次被遍历并复制到输出。

只要存在前一个 code 且字典未满，还会构造新字典条目：

```rust
let prev_string = self.dict.get_string(prev)?;
let mut new_entry = prev_string.to_vec();
new_entry.push(string[0]);
self.dict.add_string_decode(new_entry)?;
```

这里再发生一次堆分配和前缀字符串复制。KwKwK 特殊分支同样先复制
`prev_string`，再追加首字节。

因此，在字典持续增长的大部分阶段，每个数据 code 通常有：

1. 当前字符串的 owned 副本；
2. 输出 buffer 复制；
3. 新字典条目的 owned 前缀副本。

其中第 1、3 项是额外堆分配；第 3 项还会长期占用字典内存，直到下一次 reset 或
decoder drop。LZW 字符串长度随匹配推进增长，所以这些成本与解码字节数量相关，
不是每个 code 的常数小开销。

`decoder.rs:138-147` 对 `is_full()` 的重复检查只是小的冗余；与上述分配和复制
相比，它不应被当作主要瓶颈。

## 5. 输出 buffer 与 OxiGeo 边界

`decoder.rs:11-19` 将初始输出 reservation 限制为 64 KiB，`decoder.rs:63-65`
取 `expected_size` 与该上限的较小值。注释明确说明这是防止不可信 framing 提供
巨大 `expected_size` 造成资源耗尽的保护。

这个安全目标本身合理，问题在于 API 只有 owned `Vec<u8>` 一种输出形态：

1. 小于等于 64 KiB 的输出也要新建并最终 drop 一个 `Vec`；
2. 更大的输出从 64 KiB 开始按 `Vec` 增长策略扩容；
3. OxiGeo 已经有确切长度的 caller-owned `dst`，却不能把它传给 LZW decoder。

OxiGeo 的 `decompress_into_partial()` 中，未压缩、PackBits 和 DEFLATE 都有
direct-into 路径；LZW 落入通用 `_` 分支：

```rust
let decoded = decompress(src, compression, dst.len())?;
dst[..decoded.len()].copy_from_slice(&decoded);
```

这使 LZW 的实际路径变成：

```text
compressed input
  -> oxiarc owned output Vec
  -> OxiGeo copy into caller dst
```

OxiGeo 这层不是 `oxiarc-lzw` 内部慢的根因，但它把依赖内部已经复制出的完整
payload 又复制一次，并增加一个 block 级临时对象。对大 block 而言，这是稳定
的放大项。

## 6. bit reader 不是当前主因

`bitstream_msb.rs:35-73` 的 reader 每次按当前 code width 取码：

1. 逐字节填充 `u32` buffer；
2. 检查剩余位数；
3. 移位和掩码取值；
4. 更新计数。

这是安全、直观的实现，但没有做批量取码、查表或更深的输入缓冲优化。它解释
了一部分 CPU 分支和循环开销；与字典 reset、每个 code 的堆分配、多轮字节复制
以及输出增长相比，它不是当前 profile 的第一主因。

TIFF 的 MSB-first、early change 和 ClearCode 语义本身也不是性能问题的解释。
这些是格式要求；高性能实现同样必须遵守，只是可以用更紧凑的状态表示来实现。

## 7. profile 证据与排除项

P37 应用层优化后，对完整运行的不同时间窗采样得到：

- 10 秒宽窗口：worker samples 8583 中，`oxiarc_lzw::decompress` 约 5948，
  占 69.3%；predictor reverse 约 9.7%；平均采样合计约 10.3%，但在后续
  focused window 中只约 0.9%。
- 6-9 秒 focused window：worker samples 2578 中，LZW 解码链路约 81.5%，
  predictor reverse 约 11.1%。

采样中 LZW 聚合热区可见分配、释放、realloc 以及字典 reset 相关符号，这与
源码结构一致。不同窗口的比例受执行阶段影响，但两个窗口都显示 LZW 是最大项。

以下项不是当前 LZW 热区的合理解释：

1. 项目侧重复解码：此前已用调试证据确认 block cache 首次 miss 后没有重复
   解压同一 block。
2. P37 优化后的平均采样：focused window 中占比已降到约 0.9%，不是当前大头。
3. raw bytes 到 `f64` 的项目侧转换：P37 已为 cache 命中路径增加专用转换。
4. predictor reverse：它是明确的第二热点，约一成，但在 profile 中与
   `oxiarc_lzw::decompress` 分离，不应混入 LZW 内部结论。
5. 坐标变换和 gzip 输出：采样占比不足 1%。

即使完全消除 LZW 内部开销，predictor reverse 仍会留下约一成左右的独立成本；
反过来说，仅优化 predictor 也无法触及当前约七到八成的 LZW 聚合热区。

## 8. 高性能实现方向

本节只描述后续评估方向，不构成本阶段实施决定。任何依赖替换、fork 或 API
变更都需要单独授权、基准计划和输出一致性验收。

一个面向 TIFF GeoTIFF 读取的解码器应满足：

1. 解码字典使用固定大小紧凑条目，例如 `{prefix_code, final_byte, length}`；
   初始字节项可以用哨兵 prefix 表示。
2. 解码器不构建、不清理编码专用的反向 `HashMap`。
3. 输出直接写入 caller-owned slice；对不可信文件仍保留按实际解码量逐步写入
   和上限检查，而不是简单相信 `expected_size` 一次性分配。
4. 根据 code 的 prefix 链反向展开字符串，可使用复用的 scratch buffer 或栈；
   输出每个解码字节仍需写一次，但不为当前字符串和新字典条目重复分配
   owned `Vec<u8>`。
5. decoder 可跨 block 复用；reset 只恢复少量标量和固定表初始项。
6. 保持 TIFF MSB-first、early change、ClearCode、EOI、short block 和尾部
   截断语义，并以现有 C++/Rust payload 全量对比作为验收。

这条路线的目标不是改变 LZW 语义，而是把“完整字符串对象 + 中间 owned Vec +
边界复制”的实现方式替换为紧凑状态机和 direct-to-destination 输出。

## 9. 结论

`oxiarc-lzw 0.4.0` 慢的主要原因是实现形态：它把 LZW 字典当作完整字节串集合，
把解码结果当作 owned `Vec`，并且通过 one-shot API 让每个 GeoTIFF block 反复
承担对象构造、字典初始化、reset 和 teardown。OxiGeo 的通用解压边界又增加一次
完整 payload 复制。

因此，仅优化项目应用层的循环或类型转换无法继续显著降低当前最大热区；后续
收益主要取决于 LZW 解码实现或其 API 边界。相反，只优化 MSB bit reader 或移除
重复 `is_full()` 检查，预期只能得到有限改善。
