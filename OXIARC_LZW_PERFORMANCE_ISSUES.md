# oxiarc-lzw 自身性能问题说明

## 1. 分析范围

本文基于 `oxiarc-lzw 0.4.0` 的公开源码，说明该 crate 当前解码路径的性能
表现和可能产生开销的源码原因。分析对象包括：

- `src/lib.rs`
- `src/decoder.rs`
- `src/dictionary.rs`
- `src/bitstream_msb.rs`

本文只做现状分析，不包含运行基准结论，也不讨论正确性差异。

## 2. 总体表现

从源码结构看，`oxiarc-lzw` 的解码开销主要来自以下几个方面：

| 类别 | 当前表现 | 潜在结果 |
| --- | --- | --- |
| 解码器生命周期 | one-shot API 每次调用新建并销毁 decoder | 大量独立输入流会重复承担构造和 teardown |
| 字典初始化 | 对象构造、`decode()` 入口和 ClearCode 都可能触发 reset | 初始状态被重复建立 |
| 字典存储 | 每个字典项都是完整 `Vec<u8>` | 字典增长伴随多次堆分配和释放 |
| 反向查找表 | 解码路径也构建编码用 `HashMap<Vec<u8>, u16>` | 解码承担了不参与解码查找的对象成本 |
| 单个 code 处理 | 当前字符串和新字典项分别复制 | 每个 code 可能触发多次分配和内存复制 |
| 输出缓冲 | 输出始终是 owned `Vec<u8>`，初始预留最多 64 KiB | 大输出会经历多次扩容 |
| 位流读取 | 逐 code、逐字节填充小缓冲 | 有稳定分支和循环开销，但不是主要结构性开销 |

这些行为对输入形态敏感。独立流数量越多、ClearCode 越频繁、LZW 字符串越长、
输出越大，分配、复制和释放的占比通常越高。

## 3. one-shot API 的生命周期开销

`lib.rs:106-108` 的 `decompress()` 每次调用都会创建新的 `LzwDecoder`：

```rust
pub fn decompress(data: &[u8], expected_size: usize, config: LzwConfig) -> Result<Vec<u8>> {
    let mut decoder = LzwDecoder::new(config)?;
    decoder.decode(data, expected_size)
}
```

`decompress_tiff()` 在 `lib.rs:159-160` 直接转发到该函数。因此，每个被调用的
压缩输入都会经历同一套对象生命周期：

1. 创建 `LzwDecoder`；
2. 创建 `LzwDictionary`；
3. 建立初始码表和反向查找表；
4. 解码；
5. drop 字典、反向查找表和解码输出。

这带来两类固定成本。

第一类是初始化成本。`LzwDictionary::new()` 位于 `dictionary.rs:27-39`，它会先
分配外层 `table` 容量，再调用 `reset()` 填充初始状态。

第二类是 teardown 成本。字典中的每个动态字符串、反向 `HashMap` 中的每个 key，
以及解码输出 `Vec`，都需要在 decoder 离开作用域后释放。对于短输入，这些固定
成本可能比解码本身更明显；对于大量独立输入，成本会按调用次数累计。

当前公共 one-shot API 没有跨调用保留解码器状态的形式，因此调用方无法通过该
API 避免重复构造和销毁。

## 4. 字典 reset 的表现

### 4.1 初始状态被重复建立

`dictionary.rs:27-39` 显示，`LzwDictionary::new()` 构造对象后立即调用
`reset()`。随后 `decoder.rs:57-60` 中，`LzwDecoder::decode()` 入口又无条件调用：

```rust
self.dict.reset();
```

对同一个新建 decoder 的第一次 `decode()` 来说，入口 reset 会丢弃构造阶段刚
建立的初始状态，并再次完整建立一遍。

TIFF 模式下，输入流还常常以 ClearCode 开始。`decoder.rs:88-99` 处理 ClearCode
时同样调用 `self.dict.reset()`。因此，一个从 ClearCode 开始的 one-shot 输入
可能依次经历：

```text
new() 内部 reset
decode() 入口 reset
流内 ClearCode reset
```

其中流内 ClearCode 是格式状态转换的一部分；从对象状态看，前两次 reset 发生
在没有任何字典增长之前，后者建立了相同初始状态。

### 4.2 reset 本身包含多次分配

`dictionary.rs:43-61` 的 reset 流程是：

```rust
self.table.clear();
self.reverse.clear();
self.current_bits = self.config.min_bits;

let clear_code = self.config.clear_code();
for i in 0..clear_code {
    let byte_seq = vec![i as u8];
    self.table.push(byte_seq.clone());
    self.reverse.insert(byte_seq, i);
}

self.table.push(Vec::new());
self.table.push(Vec::new());
self.next_code = self.config.first_code();
```

对 TIFF 配置而言，`clear_code` 是 256。一次 reset 至少伴随：

1. 256 个单字节 `Vec<u8>` 分配；
2. 256 次单字节 `Vec<u8>` clone；
3. 256 次 `HashMap` insert；
4. 两个特殊码各占用一个外层表项；其空 `Vec<u8>` 本身不进行堆分配。

如果这不是对象首次 reset，`table.clear()` 和 `reverse.clear()` 还会先释放此前
积累的所有动态字符串和 `HashMap` key。

因此，reset 的成本由两部分组成：

1. 释放旧字典；
2. 重新创建 256 个基础条目和反向查找表。

ClearCode 出现在输入中间时，这两部分都会发生。ClearCode 越频繁，字典重建
成本越高；同时每次重建后的字典又会重新经历增长过程。

## 5. 字典数据结构的开销

### 5.1 表项保存完整字节串

`dictionary.rs:12-18` 的核心结构是：

```rust
pub struct LzwDictionary {
    table: Vec<Vec<u8>>,
    reverse: HashMap<Vec<u8>, u16>,
    config: LzwConfig,
    next_code: u16,
    current_bits: u8,
}
```

`table` 中的每个 code 都对应一个完整 owned 字节串。LZW 解码过程中，新条目
通常由“前一个字符串 + 当前字符串首字节”组成；当前实现会为这个组合后的结果
再创建一个独立 `Vec<u8>`。

这种存储形态的表现是：

1. 字典增长与堆分配数量直接相关；
2. 字符串越长，每次新增条目的复制量越大；
3. reset 或 decoder 销毁时需要逐项释放；
4. 外层 `Vec<Vec<u8>>` 和内部各 `Vec<u8>` 分散分配，缓存局部性较弱。

### 5.2 解码路径承担反向查找表成本

`reverse` 的类型是 `HashMap<Vec<u8>, u16>`。注释在 `dictionary.rs:15` 标明其
用途是编码方向的 string-to-code 查找，`find_code()` 是它的读取入口。

解码主循环没有调用 `find_code()`，也没有以字节串作为 key 查询反向表；但
`reset()` 仍会把 256 个单字节字符串插入该表，decoder 销毁或 reset 时也要清理
这些 key。

因此，解码路径的表现是：反向表参与对象构造、内存占用和释放，却不参与解码
主循环的有效查找。

## 6. 解码主循环中的分配与复制

`decoder.rs:113-132` 先为当前 code 取得字节串。常规分支是：

```rust
let string = self.dict.get_string(code)?.to_vec();
```

这一步把字典中的 borrowed slice 复制成 owned `Vec<u8>`。

随后 `decoder.rs:134-135` 输出字符串：

```rust
output.extend_from_slice(&string);
```

这一步再次遍历同一字节串，并复制到输出缓冲。

当存在前一个 code 且字典未满时，`decoder.rs:137-148` 构造新条目：

```rust
let prev_string = self.dict.get_string(prev)?;
let mut new_entry = prev_string.to_vec();
new_entry.push(string[0]);
self.dict.add_string_decode(new_entry)?;
```

这里又复制一次前一个字符串，并追加一个字节，然后把新的 owned `Vec<u8>` 存入
字典。

于是，在字典未满的常规阶段，一个 code 的典型内存动作是：

1. 当前字符串复制为临时 `Vec<u8>`；
2. 当前临时字符串复制到输出；
3. 前一个字符串复制为新字典条目；
4. 新字典条目继续保存在字典中；
5. 当前临时字符串在下一次迭代或作用域结束后释放。

KwKwK 分支也会复制前一个字符串并追加其首字节，只是来源和常规新条目不同。

这些动作意味着开销不完全由输入 code 数量决定，还与每个 code 对应的字符串
长度有关。LZW 匹配越充分，单个 code 展开的字节数越多，字符串复制量也会随之
增加。

`decoder.rs:138-147` 中 `is_full()` 被连续检查两次。这是主循环中的小冗余；与
上述分配、复制和释放相比，它对整体表现的影响较小。

## 7. 输出缓冲的表现

`decoder.rs:11-19` 定义初始输出预留上限：

```rust
const MAX_INITIAL_CAPACITY: usize = 64 * 1024;
```

`decoder.rs:63-65` 使用：

```rust
let mut output = Vec::with_capacity(expected_size.min(MAX_INITIAL_CAPACITY));
```

当 `expected_size` 小于等于 64 KiB 时，输出通常会一次性预留。当预期输出更大
时，初始预留固定为 64 KiB，后续依赖 `Vec` 的按需扩容策略。

扩容时通常伴随：

1. 新缓冲分配；
2. 旧内容复制；
3. 旧缓冲释放。

因此，输出越大，`Vec` 越可能经历多次容量阶梯。即使调用方能提供输出长度，
当前 `decode()` 的返回形式仍是 owned `Vec<u8>`，解码过程先写入这个 owned
缓冲。

还有一个边界行为：`decoder.rs:155-158` 在最后超过 `expected_size` 时调用
`truncate()`。截断本身不释放容量，但可以丢弃最后一个 code 展开出的多余字节。

## 8. 位流读取开销

`bitstream_msb.rs:35-73` 的 `MsbBitReader` 按如下方式读取 code：

1. 检查 code width；
2. 循环读入字节并填充 `u32` buffer；
3. 检查是否有足够位；
4. 移位、掩码并取出 code；
5. 更新剩余位数和总读取位数。

该实现每次只读取一个 code，并根据当前 `bits_in_buffer` 逐字节补齐。它会产生
稳定的循环和分支开销。

不过，与字典部分相比，位流读取没有大量逐 code 堆分配，也不随 LZW 字符串长度
增加复制量。它的开销主要与压缩输入长度和 code 数量相关。因此，位流读取可以
解释一部分 CPU 时间，但不足以解释字典 reset、字符串分配、多次复制和大输出
扩容带来的内存流量。

## 9. 对输入形态的敏感性

同一实现在不同输入形态下的开销分布会明显不同。

### 9.1 大量独立小输入

每个 one-shot 调用都会创建和销毁 decoder。输入本身较短时，构造初始字典、
构建反向表、释放对象等固定成本在总耗时中的占比会更高。

### 9.2 频繁 ClearCode

每次 ClearCode 都会释放已增长字典，并重新建立 256 个基础条目和反向表。若
输入中多次出现 ClearCode，重复初始化和释放会成为明显成本。

### 9.3 长匹配序列

字典项保存完整字节串，主循环又复制当前字符串和前一个字符串。随着 LZW 字符串
长度增长，分配大小和复制量随之增加。

### 9.4 大输出

初始输出预留最多 64 KiB。更大的输出需要依赖 `Vec` 扩容，可能产生多次分配、
复制和释放。

### 9.5 字典接近上限

TIFF LZW 的 code 宽度上限为 12 bit。接近上限后，新增条目数量减少，但已有字典
中积累的 owned 字符串仍然存在；下一次 ClearCode 或 decoder 销毁时，这些对象
仍需释放。

## 10. 现状归纳

`oxiarc-lzw 0.4.0` 解码路径的主要性能表现可以归纳为：

1. one-shot API 将对象构造、字典初始化、解码和对象销毁绑定在每次调用上；
2. 初始字典状态在新建对象、`decode()` 入口和流内 ClearCode 处可能被重复建立；
3. 字典项是完整 owned 字节串，反向编码查找表也随解码对象一起构建和清理；
4. 常规 code 处理包含当前字符串复制、输出复制和新字典条目复制；
5. 输出固定经过 owned `Vec<u8>`，大输出受扩容策略影响；
6. MSB 位流读取存在逐 code 分支和循环开销，但从源码结构看属于次要因素。

这些表现共同导致解码过程中的分配次数、内存复制量和对象释放次数偏高；在
独立流、长字符串、大输出或频繁 ClearCode 的输入形态下，相关成本更容易被放大。
