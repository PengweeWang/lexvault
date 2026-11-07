# 有道 YDIC 离线词库格式

状态：实验性本地导入

## 数据来源

本项目不包含、不分发有道词典数据。

转换工具只处理用户自行安装或备份后解包得到的：

```text
localdicts/
├── EC.ifo
├── EC.idx
├── CE.ifo
├── CE.idx
├── basicEC.ydic
├── 21EC.ydic
├── phrase.ydic
├── basicCE.ydic
└── newCE.ydic
```

原始安装程序及词典内容版权归网易有道所有，使用和再分发前应自行确认授权。

安装包可以静态解包，无需运行：

```bash
7z x YoudaoDictFull.exe -o/tmp/youdao-outer '$PLUGINSDIR/dict_local.7z'
7z x /tmp/youdao-outer/'$PLUGINSDIR'/dict_local.7z -o/tmp/youdao-dict
```

当前测试版本为 `6.3.69.8341`。本地转换得到：

```text
528,811 条词目
En2Cn 352,430
Cn2En 176,381
损坏记录 2
SQLite 约 205 MB
```

## IFO

IFO 是 GBK 编码的类 StarDict 文本文件：

```text
YodaoDict's dict ifo file
version:1.0.2
WordCount:352431
Lang:En2Cn
DictCount:3
DictName:基本释义
Author:YOUDAO DESKDICT INSIDE BASIC
DictFileName:basicEC.ydic
```

`WordCount` 决定索引记录数量。`DictFileName` 的顺序决定扩展引用编号：

```text
索引 0：主词典
索引 1："ex":"1#..." 指向的扩展词典
索引 2："ex":"2#..." 指向的扩展词典
```

## IDX

每个 IFO 对应一个 IDX。

前：

```text
WordCount * 12
```

字节为固定 12 字节记录，全部为 little-endian `u32`：

```rust
struct IndexRecord {
    word_offset: u32,
    data_offset: u32,
    data_length: u32,
}
```

- `word_offset` 是词头池中的开始位置。
- 下一条记录的开始位置就是当前词头的结束位置。
- `data_offset` 是主 YDIC 文件中的正文偏移。
- `data_length` 是正文长度。

索引记录之后是词头池。词头池按整字节 XOR `0x10`，内容为连续 UTF-8 词头，词头边界完全由 `word_offset` 决定。

## YDIC

YDIC 文件整体逐字节 XOR `0x10`：

```text
decoded_byte = encoded_byte ^ 0x10
```

IDX 的 `(data_offset, data_length)` 直接定位到解码后的 JSON 对象。

普通词条示例：

```json
{
  "p": "ˈæpl",
  "e": [
    "n.## 苹果；家伙"
  ],
  "ex": "1#1783127#1056",
  "ex": "2#4887333#1014"
}
```

字段：

| 字段 | 含义 |
|---|---|
| `w` | 部分词条的显式词头 |
| `p` | 音标 |
| `e` | 释义 |
| `c` | 时态、复数、比较级等词形 |
| `ex` | 扩展词典引用 |
| `pr` | 短语 |
| `o` | 原词或关联词 |

`e` 的常见格式：

```text
词性##释义
```

`c` 的常见格式：

```text
过去式##abduced$$
```

`ex` 格式：

```text
<dictionary-index>#<offset>#<length>
```

例如：

```text
1#1783127#1056
```

表示读取 IFO 中第二个词典文件的偏移 `1783127`、长度 `1056`。

## 容错

原数据存在少量非严格 JSON：

- 少数对象缺少最后一个 `}`
- 字符串内包含未转义的制表符或换行
- 同一个对象存在多个重复 `ex` 键

转换器会：

- 补全缺失的对象结束符
- 转义字符串中的 ASCII 控制字符
- 在 JSON 解析前单独扫描全部 `ex` 引用
- 对无法恢复的正文记录计数并跳过

## 输出 SQLite

输出表与 LexVault 当前只读查询核心兼容：

```sql
CREATE TABLE stardict (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    word TEXT NOT NULL,
    sw TEXT NOT NULL,
    phonetic TEXT,
    definition TEXT,
    translation TEXT,
    pos TEXT,
    collins INTEGER DEFAULT 0,
    oxford INTEGER DEFAULT 0,
    tag TEXT,
    bnc INTEGER,
    frq INTEGER,
    exchange TEXT,
    detail TEXT,
    audio TEXT,
    source TEXT,
    lang TEXT
);
```

同时创建：

```sql
CREATE INDEX sd_1 ON stardict(word COLLATE NOCASE);
CREATE INDEX stardict_3 ON stardict(sw, word COLLATE NOCASE);
```

`En2Cn` 释义写入 `translation`，`Cn2En` 释义写入 `definition`。

完整原始 JSON、扩展词典和短语保存在 `detail` 中，避免转换时丢失信息。

## 已知限制

- 依赖用户自行合法获得并解包的数据。
- 原数据中存在重复词头，转换结果保留每个索引记录。
- 极少数损坏记录会跳过。
- 当前不生成 Collins、Oxford、BNC 或 FRQ 排名。
- 音标主要来自有道基础词典和扩展词典。
- 输出数据库较大，但查询时 SQLite 页缓存占用仍然较低。
