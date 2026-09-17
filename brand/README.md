# Yunova 品牌图标

图标不是手绘文件，而是由 `generate.py` 里的参数化几何生成的：一颗三角新星，
向下的长光芒同时构成字母 **Y** 的竖笔 —— 一个形状同时承载 "Yu" 和 "nova"。
右上角的四角小星是 AI 的暗示，始终从属于主标记。

这样做的原因是一致性：favicon、应用磁贴、Windows Store 图标和 macOS `.icns`
全部从同一段定义渲染，不会各自漂移；改比例只需要改一处参数。

## 重新生成

```bash
cd brand && python3 generate.py
```

需要 Pillow（打包 `.ico` / `.icns`）和 bun（首次运行会自动装
`@resvg/resvg-js` 来栅格化 SVG）。中间产物写入 `brand/build/`（不提交），
最终资源覆盖式写入：

- `web/public/` — `logo.svg`/`.png`、`favicon.svg`/`.png`/`.ico`、
  `apple-touch-icon.svg`/`.png`、`logo-mark.svg`
- `desktop/icons/` — Tauri 打包所需的整套尺寸，含 `icon.ico` 与 `icon.icns`

## 各资源的用途

| 资源 | 用途 | 注意 |
| --- | --- | --- |
| `logo.svg` | 站内界面主图标（侧栏、登录页、消息头像） | 界面统一引用 SVG，任意 DPI 都锐利 |
| `logo.png` | 512px 位图，给不吃 SVG 的外部场景（OG 图、第三方目录） | |
| `favicon.svg` | 浏览器标签页首选 | 小尺寸专用裁切：圆角更小、图形更大 |
| `favicon.ico` | 旧浏览器与 Windows 固定磁贴 | 只含 16/32/48px，更大的交给 SVG/PNG |
| `apple-touch-icon.png` | iOS/Android 添加到主屏 | **方形不透明**，系统自己套圆角遮罩；若带透明圆角会被合成到黑底而露出黑角 |
| `logo-mark.svg` | 单色无底标记，用于印刷、水印、可换色场景 | |

## 品牌色

磁贴渐变 `#9A6BFF → #6D3BD1 → #2C0B66`，与界面 `--primary` 同一色系；
单色标记用 `#6D3BD1`（与 `web/index.html` 的 `theme-color` 一致）。
