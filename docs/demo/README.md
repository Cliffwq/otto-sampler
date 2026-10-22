# README 演示动画

`index.html` 是动画外框；`generate.cjs` 使用当前应用前端和隔离的示例数据，生成 `../images/index-search-demo.gif`。它不会启动 Tauri、扫描真实目录或读写用户数据库。搜索结果是这几组示例查询的预设结果，不是搜索算法测试。

在仓库根目录执行：

```powershell
npm ci
node docs/demo/generate.cjs
```

需要 Microsoft Edge，以及 PATH 中可用的 FFmpeg。Playwright 随 `npm ci` 安装。PNG 分镜保存在运行时输出的系统临时目录，GIF 本身可直接在 README 中显示，不需要这些生成工具。
