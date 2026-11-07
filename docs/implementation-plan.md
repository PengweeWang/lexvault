# LexVault 实施方案

状态：已确认，core/CLI/TUI/GUI 首版已实现  
更新日期：2026-11-07

## 1. 项目目标

LexVault 是一个 Rust 编写的离线英汉词典，提供：

- 有道桌面词典转换后的 SQLite 数据
- 只读 SQLite 查询
- 一次性 CLI 查词
- `ratatui` TUI
- Ely GPUI Components 桌面 GUI

安装包不包含有道安装程序或转换后的词典数据库。

## 2. 已确认决策

| 项目 | 决策 |
|---|---|
| 默认数据源 | 有道桌面词典 6.3.69.8341 |
| 正式数据文件 | `youdao.db` |
| 数据处理 | 用户本地提取和转换 |
| 数据库访问 | 只读 |
| 是否使用 ECDICT | 否 |
| 是否使用 MDX | 否 |
| 是否使用 `.aaidx` | 否 |
| CLI TUI 入口 | `lexvault --tui` |
| 普通查词 | `lexvault <单词>` |
| GUI | `lexvault-gui` |
| 安装包是否包含词典 | 否 |
| 查询核心内存目标 | 低于 10 MiB |

## 3. 数据来源

测试安装程序：

```text
YoudaoDictFull.exe
版本 6.3.69.8341
```

安装程序、词库正文和转换数据库的版权归网易有道所有。LexVault 不自动下载、不内置、不随安装包分发这些数据。

安装程序可以静态解包：

```bash
7z x YoudaoDictFull.exe -o/tmp/youdao-outer '$PLUGINSDIR/dict_local.7z'
7z x /tmp/youdao-outer/'$PLUGINSDIR'/dict_local.7z -o/tmp/youdao-dict
```

词库文件位于：

```text
/tmp/youdao-dict/localdicts/
```

格式细节见 [youdao-ydic-format.md](youdao-ydic-format.md)。

## 4. 本地转换

工具：

```bash
lexvault-youdao-convert
```

命令：

```bash
lexvault-youdao-convert \
  --input /tmp/youdao-dict/localdicts \
  --output <固定数据目录>/youdao.db
```

转换内容：

- 解析 `EC.ifo` 和 `CE.ifo`
- 解析 `.idx` 固定 12 字节索引
- 解码 XOR `0x10` 的词头池和 `.ydic`
- 合并基础词典、21世纪大英汉词典、新汉英大辞典和短语数据
- 容错处理少量非严格 JSON
- 写入 LexVault 兼容的 SQLite

验证结果：

```text
总词目：528,811
En2Cn：352,430
Cn2En：176,381
独立词头：508,081
带音标词目：271,197
损坏并跳过：2
数据库大小：约 205 MB
```

输出数据库已存在时，转换工具拒绝覆盖。

## 5. 固定数据目录

Linux：

```text
$XDG_DATA_HOME/lexvault/dictionaries/youdao-6.3.69.8341/youdao.db
默认 ~/.local/share/lexvault/dictionaries/youdao-6.3.69.8341/youdao.db
```

macOS：

```text
~/Library/Application Support/LexVault/dictionaries/youdao-6.3.69.8341/youdao.db
```

Windows：

```text
%LOCALAPPDATA%\LexVault\dictionaries\youdao-6.3.69.8341\youdao.db
```

开发环境可使用 `LEXVAULT_DATA_DIR` 覆盖平台数据根目录。

## 6. 首次启动

CLI、TUI 和 GUI 共用 `DataManager`。

启动流程：

```text
检查 youdao.db
  ├── 存在：验证大小、表和词条数，只读打开
  └── 缺失：显示转换命令和固定目标路径，退出
```

不会自动下载 ECDICT 或其他数据。

缺失提示必须包含：

- `lexvault-youdao-convert` 命令
- 解包后的 `localdicts` 参数
- 当前平台完整 `youdao.db` 路径
- 词典版本和文件名

## 7. SQLite 只读配置

查询连接：

```rust
OpenFlags::SQLITE_OPEN_READ_ONLY
```

打开后：

```sql
PRAGMA query_only = ON;
PRAGMA cache_size = -2000;
PRAGMA mmap_size = 0;
```

不创建表、索引、WAL、SHM 或 journal。

## 8. 数据库结构

转换器输出表：

```sql
CREATE TABLE stardict (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    word TEXT COLLATE NOCASE NOT NULL,
    sw TEXT COLLATE NOCASE NOT NULL,
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

索引：

```sql
CREATE INDEX sd_1 ON stardict(word COLLATE NOCASE);
CREATE INDEX stardict_3 ON stardict(sw, word COLLATE NOCASE);
```

`En2Cn` 释义写入 `translation`，`Cn2En` 释义写入 `definition`。

完整原始 JSON、扩展词典和短语保存在 `detail`。

## 9. Workspace

```text
lexvault/
├── crates/
│   ├── lexvault-core/
│   ├── lexvault-cli/
│   ├── lexvault-gui/
│   └── lexvault-youdao-convert/
├── docs/
├── README.md
└── Cargo.toml
```

`lexvault-core` 不依赖 GUI 或网络。

## 10. CLI

```bash
lexvault apple
lexvault aple
lexvault tui
lexvault --tui
```

- `lexvault tui` 查询单词 `tui`
- `lexvault --tui` 启动 TUI
- 无参数只显示帮助
- 精确查询成功时不输出日志
- 有问题时才写入 stderr

## 11. 精确查询

阶段：

1. 原始词头区分大小写的精确匹配（`COLLATE BINARY`）
2. 原始词头忽略大小写的精确匹配
3. 规范化词头 `sw` 精确匹配
4. 最近词

同一规范化词头存在大小写不同的词条时，CLI 优先返回与输入大小写完全一致的词条。

最近词使用：

- 编辑距离 1 变体
- B-tree 邻域候选
- Damerau-Levenshtein
- 必要时有界全局扫描

排序：

1. 编辑距离
2. 词条质量
3. 公共前缀
4. FRQ
5. Oxford/Collins
6. 长度
7. 字典序

## 12. 分页搜索

结果阶段：

1. 原始精确匹配
2. 规范化精确匹配
3. 其余前缀匹配

使用 `(sw, word, id)` keyset 游标，不使用 `OFFSET` 和 `COUNT(*)`。

TUI：

- 根据终端高度加载首屏
- 预取约 40 条
- Down 到达末尾时加载下一页
- 只绘制可见候选
- 候选携带数据库行 `id`，释义按该 `id` 读取，避免大小写变体串行
- 无固定展示上限

## 13. TUI

```bash
lexvault --tui
```

交互：

- 输入后自动显示第一个候选
- Up/Down 移动并立即显示释义
- Esc 清空
- PageUp/PageDown 滚动释义
- Ctrl+C 退出

候选标题显示已加载数量和是否还有更多结果。

## 14. GUI

首版 GUI 已完成：

- 入口为 `lexvault-gui`
- 使用 Ely `ely-core` 组件，而非只套用原生 GPUI 样式
- Ely 固定到提交 `e0f355ac1a669c24336244bcaa8131f7f0e4f1b5`
- GPUI 使用 `gpui-pre 0.3.11`
- 输入后自动加载并显示第一个候选
- 左侧候选列表懒加载，右侧释义独立滚动
- Windows GUI 使用窗口子系统，不显示控制台
- Windows 自定义标题栏包含最小化、最大化、关闭和置顶切换
- 左栏使用显式高度约束的原生滚动容器，支持鼠标滚轮
- SQLite 查询和释义读取在独立后台线程执行
- 每次输入带查询版本，迟到的旧结果不会覆盖当前输入
- Up/Down、Ctrl+N/P 和鼠标点击均可切换候选
- 候选携带数据库行 `id`，释义严格对应当前行
- 维护最近 20 条查询，空搜索框时可从历史中重新选择
- 缺失数据库时在窗口内显示转换命令、目标路径和“重新检查”按钮

GUI 不自行解析有道格式，只读取转换后的 `youdao.db`。

Linux 的 GPUI X11 后端由 `libxkbcommon-x11-dev` 提供链接依赖；Wayland 与
字体相关系统依赖由发行版提供。

## 15. 打包

安装包包含：

- `lexvault`
- `lexvault-gui`
- `lexvault-youdao-convert`
- README
- LICENSE 和格式说明

安装包不包含：

- `YoudaoDictFull.exe`
- `dict_local.7z`
- `localdicts/`
- `youdao.db`
- 任何有道词典正文

Windows 发布由 `.github/workflows/windows-release.yml` 自动完成：

- 推送 `v*` 标签触发
- PR 和 `main` 分支推送执行 CI 构建
- `windows-latest` 构建 x64
- `windows-11-arm` 原生构建 ARM64
- 每个架构生成独立 ZIP
- 自动创建 GitHub Release 并附加 `SHA256SUMS.txt`
- 手动触发仅生成 Actions artifact，不创建 Release

## 16. 测试

- XOR、IFO、IDX 和 JSON 容错单元测试
- 真实完整有道数据转换测试
- 精确查询和最近词测试
- 分页无重复、无遗漏测试
- SQLite 只读测试
- 缺失数据库提示测试
- CLI 参数歧义测试
- TUI 首屏、懒加载、上下键释义测试

## 17. 验收

- [x] 不依赖 ECDICT
- [x] 解析有道 `.ifo/.idx/.ydic`
- [x] 输出 528,811 条词目
- [x] 输出 LexVault 兼容数据库
- [x] CLI 精确查询
- [x] CLI 最近词
- [x] `lexvault tui` 查询单词
- [x] `lexvault --tui` 启动 TUI
- [x] TUI 首屏自动显示第一个候选
- [x] TUI Up/Down 立即显示释义
- [x] TUI 分页懒加载
- [x] 安装包不包含词典数据
- [x] 首次启动固定路径提示
- [x] GUI 查词、候选、释义与历史
- [x] GUI 缺失数据库提示与重新检查
