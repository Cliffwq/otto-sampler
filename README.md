# OTTO Sampler

**简体中文** · [English](README.en.md)

OTTO Sampler 是一个 Windows 本地音视频素材浏览器。把分散在不同文件夹的素材放进同一个搜索范围，按文件名查找、试听，再直接拖到支持文件拖入的软件中使用。

## 快速开始

打开 [最新 Release](https://github.com/Cliffwq/otto-sampler/releases/latest)，在 **Assets** 中下载安装包：

- [Windows x64 安装程序（EXE）](https://github.com/Cliffwq/otto-sampler/releases/download/v0.1.0/Otto-Sampler_0.1.0_x64-setup.exe)：下载后运行，按提示安装。
- [MSI 安装包](https://github.com/Cliffwq/otto-sampler/releases/download/v0.1.0/Otto-Sampler_0.1.0_x64_en-US.msi)：另一种安装方式，二选一即可。

支持 Windows 10/11（x64），需要 Microsoft Edge WebView2 Runtime。安装后打开程序，添加素材文件夹并扫描即可开始搜索。

## 功能

- **汉字、拼音和英文字母混合搜索。** 支持多关键词、拼音片段和通配符，具体规则见下方示例。
- **多文件夹共同搜索。** 将多个文件夹保存为一个路径组合，一起搜索其中的素材；通过增量扫描更新索引。
- **筛选与排序。** 按音频、视频筛选，按文件名、时长或修改时间排序；记住上次的排序方式。搜索结果每次加载 200 条，滚动到底部继续加载。
- **拖拽与预览。** 音频和视频都可以从列表直接拖到其他软件中。单击音频试听，双击视频用系统默认程序打开。
- **侧边收纳。** 把窗口拖到屏幕左右边缘，移开鼠标后收起，鼠标移回边缘即可召出。也可以让窗口保持置顶。
- **四套主题。** 配色参考 Akai MPC60、MPC2000XL、E-mu SP-1200 和 Roland SP-404MKII，每套都有日间、夜间版本，也可跟随系统。默认使用 MPC60。

### 索引与搜索演示

索引是一份保存在本机的素材目录，记录文件名和位置，让多个文件夹里的素材可以一起搜索。文件仍放在原处。以后文件夹有变化，增量扫描会检查并更新这份目录；查找时，可以把汉字、拼音和英文混在一起输入，不必先把文件名改成统一的语言。

![从建立索引、新增文件后的增量扫描，到汉字、拼音和英文混合搜索的演示](docs/images/index-search-demo.gif)

示例：两个文件夹中的 6 个素材建立索引，新增一个音效后扫描补入 1 条记录，再用 `鼓 kick`、`gukick` 和 `大gu kick` 找到相同的结果。动画使用示例数据说明操作流程，不代表扫描速度。

### 主题预览

<p>
  <img src="docs/images/mpc60-light.png" width="360" alt="MPC60 日间主题，显示音频素材列表">
  <img src="docs/images/sp404-dark.png" width="360" alt="SP-404MKII 夜间主题，显示同一素材列表">
</p>

左：MPC60 日间；右：SP-404MKII 夜间。截图使用示例文件名，界面语言目前为简体中文。

## 使用

1. **添加素材目录。** 打开顶部的“文件夹＋扳手”按钮，在路径管理中输入组合名称，添加一个或多个文件夹，然后保存组合。常用组合可以设为默认。
2. **建立索引。** 在顶部选择刚保存的组合，点击增量扫描。以后添加或修改素材时，再扫描即可，不需要重新导入整个素材库。
3. **查找并使用。** 输入关键词，单击音频试听，按住列表中的文件直接拖到目标软件。目标软件需要支持该文件格式和文件拖入。
4. **调整工作方式。** 在齿轮设置中选择配色、日夜模式和启动扫描；把窗口拖到屏幕侧边即可收纳。窗口尺寸和排序偏好会在下次启动时恢复。

鼠标停在按钮上时，顶部提示区会显示它的用途。设置中的“重置文件名索引库”会清空索引，保留路径组合和原始素材，之后可以重新扫描。

扫描或重置结束后，程序会在数据库空闲空间明显偏大时尝试回收磁盘空间，不删除有效索引记录或素材；数据库忙时会留待之后处理。

### 搜索规则

搜索针对**文件名**，不分析音频内容，也不自动翻译词义。英文字母不区分大小写。

| 输入 | 匹配方式与例子 |
| --- | --- |
| `鼓` | 匹配含“鼓”的文件名，例如 `大鼓Kick.wav` |
| `gukick` 或 `鼓kick` | 汉字可用拼音代替，也可与英文字母连写；都能找到 `大鼓Kick.wav` |
| `nihaosh` | 拼音可以停在一个字的中间，能找到 `你好世界.wav` |
| `鼓 kick` | 每个关键词分别匹配不同片段，顺序不限；能找到 `Kick 大鼓.wav` |
| `ou shou` | 能找到 `ou shou.wav`，但不会把 `shou.wav` 中的 `ou` 重复算作另一个匹配 |
| `kick*dry` | 只有 `*` 是通配符，表示任意长度的内容；能找到 `Kick 01 Dry.wav` |
| `100%`、`take_01` | `%`、`_` 等其他符号按原字符匹配 |

不支持拼音首字母缩写，例如 `bj` 不会因拼音而匹配“北京”。

## 本地开发

使用 Tauri 2、Rust 和原生 JavaScript，索引存储使用 SQLite。

### 环境

- Git、Node.js LTS（含 npm）。
- Rust stable，使用 MSVC 工具链。
- Visual Studio Build Tools，安装“使用 C++ 的桌面开发”和 Windows SDK。
- Microsoft Edge WebView2 Runtime；运行界面测试还需要 Microsoft Edge。

在终端中确认能使用 `node`、`npm` 和 `cargo`。首次安装依赖和打包需要联网，构建工具会下载 npm/Cargo 依赖以及 WiX、NSIS 等打包组件。

### 获取代码并启动

```powershell
git clone https://github.com/Cliffwq/otto-sampler.git
cd otto-sampler
npm ci
npm run tauri -- dev -- --locked
```

以下命令均在仓库根目录执行。

### 测试

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib
npm run test:ui
```

Rust 测试使用临时素材和隔离数据库；界面测试使用 Playwright 与 Edge，并模拟 Tauri 接口，不操作个人素材库。它们不覆盖实际的系统拖放和安装过程。

原生侧边收纳检查可用 `tools/test-window-docking.ps1 -AppProcessId <PID>`。它会移动指定程序的窗口，请使用测试实例。

### 构建发行版

```powershell
# 生成 EXE、MSI 和 NSIS 安装包
npm run tauri -- build -- --locked

# 只生成 EXE，跳过安装包
npm run tauri -- build --no-bundle -- --locked
```

| 产物 | 默认位置 |
| --- | --- |
| 程序 | `src-tauri/target/release/tauri-app.exe` |
| MSI 安装包 | `src-tauri/target/release/bundle/msi/` |
| NSIS 安装包 | `src-tauri/target/release/bundle/nsis/` |

构建前关闭正在使用的同名 EXE，避免文件占用。上述路径以未覆盖 `CARGO_TARGET_DIR` 为准；安装包目前未签名。

### 目录与维护

| 位置 | 内容 |
| --- | --- |
| `src/` | 界面、主题、图标与交互 |
| `src-tauri/src/` | 索引、搜索、文件操作与窗口管理 |
| `src-tauri/icons/` | 程序与安装包图标 |
| `tools/` | 界面回归、原生窗口检查和空间统计 |
| `docs/images/` | README 配图 |

数据库和偏好在当前用户目录自动创建，不需要复制开发者的数据。`node_modules`、编译输出和本地调试文件不纳入 Git，README 配图随仓库保存。

修改拖拽图标时，同时更新 `src-tauri/icons/32x32.png` 与 `src/assets/drag-icon.js` 中的内嵌 PNG；界面测试会检查二者一致。

`.cargo/config.toml` 关闭调试符号和增量编译，以减少磁盘占用。需要检查工作区大小时，可运行 `powershell -NoProfile -File tools/check-size.ps1`；该工具默认以 3 GB 为预算，超出时返回退出码 2，不影响上述构建命令。
